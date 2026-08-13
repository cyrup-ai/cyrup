# LEADS-SETTLED — `DRIFT-023` and `DRIFT-040`, read on both sides

**Status** evidence record, **not an ADR**. It carries no `Status: accepted` line and decides nothing
on its own; it discharges the two-sided-read obligation that held the backlog's last two leads below
the evidence bar. It lives in `docs/adr/` because that is where the decisions it feeds live, and
because `ADR-0009`'s actionability rule admits `docs/adr/` and `docs/gap-analysis/` as the only
authorities a commit may cite. It is **not numbered**, and no future ADR may take a number from it.
**Consumed by** `ADR-0004` (Part 2 corroborates its ruling from upstream's own words and reproduces
its sqlite figure independently; ADR-0004 adopts §2.2's checkbox count as a third, leading tripwire)
and by area 05 / area 12 (Part 1's three corrections to `CFG-020`, and the 26 stale
`model-registry.ts` citations).
**Date** 2026-08-13
**Scope** `DRIFT-023` and `DRIFT-040`, both in `docs/gap-analysis/12-upstream-drift-pi-core.md`
(`:99`/`:531` and `:116`/`:687`), both labelled *lead — not yet evidenced*, both carried forward
unverified for two consecutive passes.
**Method** every upstream fact is a `git`-object read at a named tag (`git show <tag>:<path>`,
`git ls-tree`, `git diff`, `git merge-base --is-ancestor`); every cyrup fact is at HEAD
`72cd292` (`david/cyrup`). No working tree in either repository was modified, and no Rust or
TypeScript source was touched.

Area 12 recorded the exact commands that would settle these two rows
(`12-upstream-drift-pi-core.md:189-190`). They were run. Both leads are now closed, **and both
turned out to rest on a false premise** — which is why running them was worth more than the two
commands it cost.

---

## Summary

| | `DRIFT-023` | `DRIFT-040` |
|---|---|---|
| Filed as | `upstream-drift` · tracking · "pi is mid-refactor, track don't port" | tracking · "pi's harness v2 rearchitecture entirely unabsorbed" |
| Premise | `ModelRegistry` → `ModelRuntime` migration is **in progress** upstream | the harness v2 surface is **moving**; re-scope when pi's coding-agent migrates |
| Premise verdict | **false** — the migration completed and shipped in **v0.80.8**, three minors *before* cyrup's port baseline | **false in the direction that matters** — pi's own design doc lists coding-agent migration as an explicit **non-goal** |
| Its own diffstat claim | `model-runtime.ts +274/−82` — **confirmed exactly** | `agent-harness.ts +420/−996` ✅ · `harness-v2.md +2124/−367` ✅ · sqlite `+12598/−3479` ❌ (true: `+4,010/−1,494`) |
| Real gap? | yes, but **wholly owned by `CFG-020`** | no — nothing to port; upstream has not built it |
| Disposition | **close as superseded by `CFG-020`**; reclassify the gap `not-ported`, not `upstream-drift` | **close out of scope**, under ADR-0004 and upstream's own stated non-goal |

Both closures are consistent with, and independent of, ADR-0004. Where this document overlaps
ADR-0004 (the sqlite-node figure, the harness reachability trace) the numbers were re-derived
here from scratch and **reproduce ADR-0004 exactly**; that agreement is reported, not assumed.

---

# Part 1 — `DRIFT-023`: the refactor pi finished before cyrup started

## 1.1 What the item claims

> **upstream** — pi `9993c969`. Both `model-registry.ts` and `model-runtime.ts` still coexist in
> `pi/packages/coding-agent/src/core/` at `v0.84.1`, so the migration is in progress, not finished.
> — `12-upstream-drift-pi-core.md:545`

> **Fix** — Deferred by design; re-scope once pi finishes the migration and deletes
> `model-registry.ts`. — `:551`

The coexistence observation is **true**. The inference drawn from it is **false**, and the Fix
therefore waits on an event that will not occur.

## 1.2 The two-sided read

**Both files exist at the ported tag.** This is the check area 12's blind spot 9 prescribes
(`:1004`: "*Before assigning `upstream-drift`, run `git cat-file -e v0.83.0:<path>`*"):

```
$ git -C pi cat-file -e v0.83.0:packages/coding-agent/src/core/model-registry.ts   # exists
$ git -C pi cat-file -e v0.83.0:packages/coding-agent/src/core/model-runtime.ts    # exists
```

| | v0.83.0 | v0.84.1 |
|---|---|---|
| `model-registry.ts` | 145 lines | 157 lines |
| `model-runtime.ts` | 595 lines | 787 lines |

**`ModelRegistry` is already a pure delegating facade at v0.83.0**, and says so in its own header:

```ts
// pi @v0.83.0 packages/coding-agent/src/core/model-registry.ts:16-19
/**
 * Synchronous compatibility facade exposed to extensions.
 * Coding-agent internals use ModelRuntime directly.
 */
export class ModelRegistry {
	private readonly runtime: ModelRuntime;
```

Every one of its method bodies is `return this.runtime.X(...)` — at v0.84.1, 22 `this.runtime`
references in a 157-line class. Its consumers at v0.84.1 are
`core/extensions/runner.ts`, `core/extensions/types.ts`, `core/agent-session.ts`,
`modes/interactive/interactive-mode.ts` and `src/index.ts` — i.e. **the extension-facing API
surface and the package export**. It is a published compatibility boundary, not a legacy
leftover queued for deletion.

**The refactor shipped three minors before cyrup's baseline.** The commit `DRIFT-023` itself
cites is the commit that *created* the facade:

```
$ git -C pi log -1 --format='%h %ad %s' --date=short 9993c9690
9993c9690 2026-07-14 feat(coding-agent): replace model registry with model runtime

$ git -C pi merge-base --is-ancestor 9993c9690 v0.83.0 && echo ANCESTOR
ANCESTOR
$ git -C pi tag --contains 9993c9690 | sort -V | head -1
v0.80.8
```

So the state `DRIFT-023` describes as "migration in progress" is pi's **finished, released
steady state**, and has been since v0.80.8. `model-registry.ts` will not be deleted.

## 1.3 What actually changed in the delta — and it is not a rename

`model-runtime.ts` `+274/−82` is **confirmed exactly** (`git diff --numstat v0.83.0..v0.84.1`).
The `−82` is not a re-architecture; it is the promise-coalescing availability refresh being
replaced by sequence-guarded invalidation. The change is **behavioural**, and all of it is new
capability. At v0.84.1 (line numbers in `model-runtime.ts` @v0.84.1):

| Addition | Where | Behaviour |
|---|---|---|
| `CredentialSynchronizationError` | `:94-111` | credentials commit but the local snapshot fails to sync — a distinguishable outcome, previously silent |
| `enqueueCredentialOperation` | `:494-512` | per-provider serialization of `login`/`logout`/`setRuntimeApiKey`/`removeRuntimeApiKey` |
| `synchronizeCredentialState` | `:514-533` | recompose → scoped `refresh({providers:[id]})` → per-provider availability, as one unit |
| `refreshProviderAvailability` | `:331-386` | per-provider availability pass that patches the snapshot instead of rebuilding it |
| seq guards | `:148-150`, `:299`, `:317-322` | `availabilityRefreshSeq` / `availabilityErrorSeq` / `providerAvailabilitySeq` — stale passes drop their results and their errors |
| scoped refresh | `:690-700` | `ModelsRefreshOptions.providers` recomposes only named providers |
| `isUsingSubscription` | `:462` | `isUsingOAuth(id) && provider.auth.oauth?.isSubscription` |
| `fetchDeferred` / `cancelDeferred` | `:647`, `:661` | deferred/batch provider responses |
| cancellation throughout | `AuthOperationOptions`, `options.signal` | every auth/availability call takes an `AbortSignal` |
| `refreshOnCreate` | `:81`, `:210` | create without a catalog/availability round-trip |

`ModelRegistry` mirrored three of these outward — `complete()`, `refresh()` returning
`ModelsRefreshResult`, and `baseUrl` on `ResolvedRequestAuth`
(`git diff v0.83.0..v0.84.1 -- packages/coding-agent/src/core/model-registry.ts`, `+32/−20`).

**So the correct one-line characterisation is:** not a rename, not a re-architecture — the
rename happened at v0.80.8; the v0.84.1 delta is a **behavioural** change adding credential
operation serialization, cancellation, scoped refresh and deferred requests to an already-stable
`ModelRuntime`.

## 1.4 The cyrup side

`DRIFT-023`'s cyrup claim — "no `ModelRuntime` type; the hits are doc citations" — is
**confirmed, and understated**:

```
$ grep -rnE '^\s*(pub )?(struct|enum|trait|type|impl)\s+ModelRuntime' crates/ --include='*.rs'
(no output)
```

Every `ModelRuntime` occurrence in `crates/` is a doc comment. The hits span nine files, not the
two the item names (`cyrup-config/src/provider_compose.rs`, `auth.rs`,
`cyrup-provider/src/collection.rs`, `remote_catalog.rs`, `auth/store.rs`,
`cyrup-session-svc/src/session.rs`, `services.rs`, plus two test files). None of the v0.84.1
behaviour exists either:

```
$ grep -rn 'CredentialSynchronization\|enqueue_credential\|credential_operations' crates/ --include='*.rs'
(no output)
$ grep -rn 'is_using_subscription' crates/ --include='*.rs'
(no output — only test prose about isSubscription)
```

`cyrup_core::DeferredHandle` **is** ported (`crates/cyrup-core/src/message.rs:497-505`, correctly
cited to `v0.84.1 ai/src/types.ts:395-404` and noting the type does not exist at v0.83.0), but
there is no `fetch_deferred`/`cancel_deferred` on any provider dispatch surface. And cyrup has no
snapshot at all: `full_model_registry()`
(`crates/cyrup-session-svc/src/session.rs:2679-2720`) recomposes base + guest + built-ins on
every call, and `available_model_catalog()` (`:2726-2728`) calls it and filters.

## 1.5 Three corrections this read produces

**(a) The "+356 lines" figure repeated in two files is an arithmetic error.**
`05-cyrup-config-and-resources.md` (CFG-020 upstream para) and `PARITY-GAPS.md:636` (VL-P24) both
say the target "grew +356 lines at v0.84.1". It did not:

```
$ git -C pi diff --numstat v0.83.0..v0.84.1 -- packages/coding-agent/src/core/model-runtime.ts
274	82
$ echo $((274 + 82))
356
```

**356 is insertions + deletions read as growth.** True growth is **+192 net** (595 → 787 lines).
The file is ~32% larger, not ~60%. VL-P24's other citations are sound: `model-runtime.ts:94-111`
and `:494` are both exact at v0.84.1.

**(b) A missing dependency nobody has recorded.** The cancellation half of the v0.84.1 change
rests on `packages/coding-agent/src/utils/abort.ts` — **new at v0.84.1**, 48 lines, exporting
`operationSignal(signal?)` (`:9`) and `raceWithAbortSignal(operation, signal)` (`:14`), imported
at `model-runtime.ts:41`. Whoever schedules `CFG-020`/VL-P24 needs its behaviour first; it
appears in no area file.

**(c) A free conformance target.** `packages/coding-agent/test/model-runtime-credential-sync.test.ts`
is **375 lines, new at v0.84.1** — an executable specification of exactly the serialized,
cancellable credential behaviour `CFG-020`/VL-P24 must build. Three sibling test files also moved
(`model-runtime-auth-options` `+71/−1`, `model-runtime-modify-models-compat` `+132/−1`,
`model-runtime-cloudflare-compat` `+7/−0`). `CFG-020`'s **Verify** currently proposes an
invalidation-count assertion invented from scratch; upstream ships the suite.

## 1.6 Incidental finding — 26 unverifiable upstream citations in shipped Rust

Settling `DRIFT-023` exposes a class defect the lead did not anticipate. cyrup's doc comments
cite `model-registry.ts` at line numbers that **cannot exist at the ported tag**:

```
$ grep -rnoE 'model-registry\.ts:[0-9]+(-[0-9]+)?' crates/ --include='*.rs' | sort -u | wc -l
26
```

26 distinct citations across **9 files** — `cyrup-config/src/{auth.rs,model.rs}`,
`cyrup-ext/src/provider.rs`, `cyrup-provider/src/config_provider.rs`,
`cyrup-session-svc/src/{guest_providers.rs,session.rs}`,
`cyrup-session-svc/tests/model_registry.rs`, `cyrup-tui/src/{app.rs,auth_select.rs}`. The lowest
is `:204-214`; the highest is `:892-960`. **The file is 145 lines at v0.83.0 and 157 at
v0.84.1 — every one of the 26 is out of range.**

They were valid once:

```
$ git -C pi show 9993c9690^:packages/coding-agent/src/core/model-registry.ts | wc -l
1018
$ git -C pi cat-file -e 9993c9690^:packages/coding-agent/src/core/model-runtime.ts
(fatal: does not exist)
```

The port was written against **pre-v0.80.8** pi, when `model-registry.ts` was the 1018-line
implementation and `model-runtime.ts` did not exist. The citations were never rebased when the
port target moved to v0.83.0. In nearly every case the correct counterpart is now
`model-runtime.ts`. Concretely: `session.rs:2687` and `guest_providers.rs:2,7,48` cite
`model-registry.ts:917-940` for the guest-provider fold; that behaviour lives in
`ModelRuntime.registerProvider`/`rebuildProviders` at v0.83.0.

This is a documentation defect, not a behavioural one — but it means 26 upstream claims in
shipped code are unverifiable by anyone following them, and it is the same failure mode as blind
spot 9 (`:1004`), one layer down. **It is not filed here** (this document files nothing); it is
recorded for area 05 / area 01 to file.

## 1.7 Disposition

**`DRIFT-023` closes as superseded by `CFG-020`.** The `duplicate-of` marking was correct.
`CFG-020` (`05-cyrup-config-and-resources.md:392`, medium, effort L, *confirmed*) already owns
the gap, is already classified `not-ported` rather than `upstream-drift`, and already carries the
"read `model-runtime.ts` at v0.84.1, not v0.83.0" instruction. `DRIFT-023` adds nothing to it and
subtracts confidence by describing a migration that finished before cyrup's baseline.

**The classification was wrong and the correction matters.** `DRIFT-023` is filed
`upstream-drift`. `ModelRuntime` existed at v0.83.0 in full (595 lines); cyrup did not port it.
That is a **port omission**, and no rebase onto v0.84.1 will resolve it. This is the **eighth**
of area 12's nine hash-dated items to be re-derived and the **seventh** to prove misclassified
in exactly this way (`:1004`).

Area 12 should record: kind corrected `upstream-drift` → `not-ported`; premise refuted (facade is
permanent, refactor shipped v0.80.8); closed into `CFG-020`; ID retained so the call is
re-auditable. Area 05 and `PARITY-GAPS` should take corrections (a), (b) and (c).

---

# Part 2 — `DRIFT-040`: pi's harness v2 is a quarter-built greenfield rebuild

ADR-0004 answers the scope question (**out of scope: port the behaviour the harness pins, not the
harness**). This part answers the narrower factual questions the assignment poses, which ADR-0004
did not: *what did the rearchitecture actually change, is `docs/harness-v2.md` in pi's tree, and
what is the sqlite rebuild it mentions?* The answers **strengthen ADR-0004 with evidence from
upstream's own words**, and would have been worth having under either ruling.

## 2.1 Yes — `harness-v2.md` is in pi's tree, at both tags

`packages/agent/docs/harness-v2.md`. Present at **v0.83.0 (1,655 lines)** and **v0.84.1 (3,412
lines)**; `+2124/−367` is **confirmed exactly**. A second file, `harness-v2-test-matrix.md` (195
lines), is new at v0.84.1. Titled *"Durable AgentHarness design"*, 21 sections, 109 headings at
v0.84.1 (68 at v0.83.0).

## 2.2 What the +2124/−367 actually is: a design sketch became an executable work plan

The v0.83.0 document ends in TODOs:

```
# pi @v0.83.0 packages/agent/docs/harness-v2.md
## 18. Open questions          (three unresolved: per-lane hooks, records & replication, fork × lanes)
## 19. Testing strategy
TODO after the document is reviewed end to end.
## 20. Implementation sequence
TODO after the document is reviewed end to end.
```

At v0.84.1 §18 is gone, §19 is a three-tier testing strategy, and §20 is
**"Implementation status and work packages"** (`:3147`) — a claim-and-reserve protocol
(`docs(agent): reserve <package-id>`, one reservation per package, sync with `main`), an
exhaustive public-method ownership table, and **40 tracked work packages across eight tracks**
(F, QA, R, J, I, L, H, C/N, O).

**This is the measurement that was missing, and it is decisive:**

```
$ git -C pi show v0.84.1:packages/agent/docs/harness-v2.md | grep -cE '^- \[x\]'   →  10
$ git -C pi show v0.84.1:packages/agent/docs/harness-v2.md | grep -cE '^- \[ \]'   →  30
$ git -C pi show v0.83.0:packages/agent/docs/harness-v2.md | grep -cE '^- \[[x ]\]' →  0
```

**pi's harness v2 is 10 of 40 work packages complete — 25% — by its own tracker, which did not
exist at v0.83.0.** Unclaimed packages include `prompt` (H1), run `resume` and retries (H2),
`steer`/`followUp` (H3), `abort`/`waitForIdle` (H5), tool execution and recovery (H6/H7),
compaction (C1–C3), navigation (N1), and JSONL storage (Track J).

## 2.3 The `agent-harness.ts` "rewrite" is a scaffold reduction, not new behaviour

`+420/−996` is **confirmed exactly**, and the file **shrank from 1,084 to 508 lines**
(1084 − 996 + 420 = 508 ✓). §20's first work package explains why:

> **F0 — harden the scaffold.** … Make every other placeholder reject with
> `HarnessNotImplemented` instead of returning empty snapshots, idle state, or no-op drive/wait
> success. … Acceptance: a table-driven scaffold test covers every public method and proves no
> unfinished method reports plausible success.

```
$ git -C pi grep -c 'HarnessNotImplemented' v0.84.1 -- 'packages/agent/src'
v0.84.1:packages/agent/src/harness/agent-harness.ts:5
$ git -C pi grep -c 'HarnessNotImplemented' v0.83.0 -- 'packages/agent/src'
(no output — the symbol does not exist at v0.83.0)
```

Declared at `agent-harness.ts:74-79` @v0.84.1; thrown at `:233`, `:351`, `:356`. The largest
single edit in the surface `DRIFT-040` calls "the largest single unabsorbed surface in pi core"
is upstream **deleting functionality that looked real and replacing it with explicit
not-implemented rejections**. There is no behaviour there to be 1:1 with, because upstream
removed the appearance of it.

## 2.4 Upstream's own non-goal refutes the item's re-scope trigger

`DRIFT-040`'s Fix says re-scope "*once `git -C pi log --oneline -- packages/agent/src/harness`
goes quiet*"; `PARITY-GAPS.md:834` option (c) says "*out of scope until pi's own `coding-agent`
migrates*". pi's design document rules that migration out **in writing**:

> **Coding-agent migration.** Migrating coding-agent to `AgentHarness` is out of scope.
> Compatibility means the new JSONL repository can read supported coding-agent v3 files.
> — `harness-v2.md:41` @v0.84.1, under **## Non-goals**

> Work is limited to `packages/agent`, `packages/session-backends/sqlite-node`,
> `packages/telemetry`, and the telemetry request-option surface in `packages/ai`. Other package
> source is off limits. **In particular, this plan does not migrate `packages/coding-agent`.**
> — `harness-v2.md:3149` @v0.84.1

The event `DRIFT-040` waits for is not merely distant; it is **explicitly excluded from the plan
that would produce it**. That is upstream corroboration of ADR-0004's ruling, from a source
neither document had read.

## 2.5 The compatibility direction settles the absorbed `DRIFT-037` residue

`DRIFT-040` absorbed `DRIFT-037`'s residue — *interop with harness-v2-written sessions*. The
design document's first line fixes the direction, at **both** tags:

> **Compatibility policy.** Old coding-agent v3 JSONL sessions must open and restore idle. **This
> is the only backward-compatibility requirement.** All other formats and APIs in
> `packages/agent/src/harness` and `packages/session-backends/sqlite-node` (and their respective
> tests) may break. We do not write migrations, schema versioning, or conversion paths for
> anything else. — `harness-v2.md:3`

Restated as a goal at `:33`: "*v3 sessions load. Old coding-agent v3 JSONL files open unchanged
and restore idle.*"

cyrup writes coding-agent v3. The obligation runs **one way — harness-v2 must read cyrup's
format, not the reverse** — and upstream disclaims stability for everything else. There is no
interop work on cyrup's side, and any built against harness-v2's shapes today would be built
against formats upstream reserves the right to break. The residue closes with the item.

## 2.6 What the "sqlite rebuild" is

`packages/session-backends/sqlite-node`, **renamed from `packages/storage/sqlite-node`** (18
files at v0.83.0 → 37 at v0.84.1; the rename is visible in `harness-v2.md:3` itself, which says
`packages/storage/sqlite-node` at v0.83.0 and `packages/session-backends/sqlite-node` at
v0.84.1).

It is **one of three interchangeable backends** behind the `SessionStorage` interface
(`harness-v2.md:1571-1630`) — memory (the reference), JSONL, SQLite — with a **greenfield
schema**, explicitly not a migration of anything (`:1674-1757`): tables
`session_sequences`, `entries`, `records`, `lanes`, `lane_moves`, `facts`, `branch_entries`,
`branch_tips`, `writer_leases`. Its distinguishing designs are `writer_leases` (single-writer
enforcement via expiring fenced claims, renewed inside every write transaction) and a private
`branch_entries`/`branch_tips` read cache resting on two invariants — *every entry is in at least
one branch* and *tips are unique* — that reduce `findEntriesOnBranch` to a point lookup plus a
bounded range scan.

**The figure is refuted.** `DRIFT-040` reports `+12598/−3479`. Re-derived here independently:

```
$ git -C pi diff --shortstat -M v0.83.0 v0.84.1 -- packages/storage/ packages/session-backends/
 49 files changed, 4010 insertions(+), 1494 deletions(-)
$ git -C pi diff --shortstat --no-renames v0.83.0 v0.84.1 -- packages/storage/ packages/session-backends/
 55 files changed, 4253 insertions(+), 1737 deletions(-)
```

**+4,010/−1,494** with rename detection — overstated ~3×, and reproducing ADR-0004's figure
exactly from an independent run.

**And it ships in nothing.** The reachability trace:

```
$ git -C pi grep -l 'session-backends\|sqlite' v0.84.1 -- 'packages/coding-agent/src'
(no output)
```

`packages/agent/package.json` @v0.84.1 depends only on `pi-ai` and `pi-telemetry` — not on any
session backend. `packages/coding-agent/package.json` depends on `pi-agent-core`, `pi-client`,
`pi-protocol` — not on `session-backends`; and its `build:binary` script builds
`tui, telemetry, ai, agent, protocol, client` and **not** `session-backends`. The SQLite backend
is not in the shipped `pi` binary and is imported by no shipping code. Under the parity rule the
behaviour set it contributes to `pi` is empty — the same ground ADR-0004 decides on.

## 2.7 Disposition

**`DRIFT-040` closes out of scope**, on ADR-0004's ruling, with three reasons this document adds
that ADR-0004 did not have:

1. Upstream's design document declares coding-agent migration a **non-goal** in writing
   (`harness-v2.md:41`, `:3149`) — the item's own re-scope trigger is excluded by upstream policy.
2. The harness is **25% built** (10/40 packages) and its main class deliberately throws
   `HarnessNotImplemented` on unfinished methods. There is no behaviour to port yet.
3. Compatibility runs **one way** (`:3`): harness-v2 must read coding-agent v3. The absorbed
   `DRIFT-037` interop residue has no cyrup-side work and closes with the item.

Its three load-bearing figures are now settled: `agent-harness.ts +420/−996` ✅ confirmed;
`docs/harness-v2.md +2124/−367` ✅ confirmed; sqlite `+12598/−3479` ❌ refuted (**+4,010/−1,494**).
`DRIFT-040` ceases to be a lead.

**One note for ADR-0004's tripwire (its item 6).** That tripwire watches for `AgentHarness`
appearing in `packages/coding-agent/src`, or a coding-agent dependency on the sqlite backend.
Both are correct and both are *lagging* signals. §20's checkbox count is a **leading** one, and
costs one command:

```
git -C pi show <newtag>:packages/agent/docs/harness-v2.md | grep -cE '^- \[ \]'
```

30 open today. It reaching 0 means the rebuild is finished and pi's calculus about migrating
coding-agent can change — the signal arrives before either lagging tripwire fires, not after.
Offered as an addition to the sweep, not a change to the ADR.

---

# What the area files should now say

Nothing here edits any file. For whoever does:

**`docs/gap-analysis/12-upstream-drift-pi-core.md`**
- `DRIFT-023` (`:99`, `:172`, `:189`, `:531`) — drop **LEAD**; **closed, superseded by `CFG-020`**.
  Record: kind corrected `upstream-drift` → **`not-ported`** (`model-runtime.ts` is 595 lines at
  v0.83.0); premise **refuted** — `ModelRegistry` is a permanent extension-facing facade
  (`model-registry.ts:16-19` @v0.83.0) and `9993c9690` shipped in **v0.80.8**, so "re-scope once
  pi deletes `model-registry.ts`" waits on an event that will not occur. `+274/−82` confirmed.
  Keep the ID.
- `DRIFT-040` (`:116`, `:174`, `:190`, `:687`) — drop **LEAD**; **closed out of scope** under
  ADR-0004, plus §2.4/§2.5 above. Two figures confirmed, one refuted (**+4,010/−1,494**). The
  absorbed `DRIFT-037` residue closes with it. Keep the ID.
- Blind spot 8 (`:1002`) and blind spot 9 (`:1004`) — both discharged; **eight of nine**
  hash-dated items now re-derived, **seven** misclassified. Blind spot 4 (`:998`) — the
  `Storage`-as-swappable-interface worry is answered: the interface exists but the sqlite backend
  it was raised about is imported by nothing shipping (§2.6).
- `## Leads — not yet evidenced` (`:189-190`) — **now empty**; the section can be removed.

**`docs/gap-analysis/05-cyrup-config-and-resources.md`** — `CFG-020` absorbs `DRIFT-023` and takes
three corrections: replace "+356 lines" with **+274/−82, net +192 (595 → 787)**; add
`packages/coding-agent/src/utils/abort.ts` (new at v0.84.1, 48 lines) as a prerequisite; point
**Verify** at `test/model-runtime-credential-sync.test.ts` (375 lines, new at v0.84.1) instead of
an invented assertion. Severity, kind and effort are already right and do not change.

**`docs/gap-analysis/PARITY-GAPS.md`** — VL-P24 (`:636`): same "+356" correction; its
`model-runtime.ts:94-111` and `:494` citations are exact and stand. VL-P22 / `:834` / `:889`: the
two leads are settled; the harness rows close per ADR-0004.

**`docs/gap-analysis/00-residual-ledger.md`** (`:288`, `:290`, `:590`) and **`README.md:120`** —
both leads settled; the "two leads" language retires.

**New, not yet filed anywhere** — the 26 stale `model-registry.ts:NNN` citations across 9 crates
(§1.6). A doc-only correction, but 26 upstream claims in shipped Rust currently point at a file
version that predates cyrup's own port baseline.

---

# Appendix — every command, re-runnable

```sh
PI=/Users/davidmaple/cyrup.ai/pi; CY=/Users/davidmaple/cyrup.ai/cyrup   # cyrup HEAD 72cd292

# --- DRIFT-023 ---
git -C $PI cat-file -e v0.83.0:packages/coding-agent/src/core/model-runtime.ts   # exists => not drift
git -C $PI diff --numstat v0.83.0..v0.84.1 -- packages/coding-agent/src/core/model-{registry,runtime}.ts
git -C $PI show v0.83.0:packages/coding-agent/src/core/model-registry.ts | sed -n '16,25p'  # facade
git -C $PI merge-base --is-ancestor 9993c9690 v0.83.0 && git -C $PI tag --contains 9993c9690 | sort -V | head -1
git -C $PI show 9993c9690^:packages/coding-agent/src/core/model-registry.ts | wc -l          # 1018
git -C $PI diff --numstat v0.83.0..v0.84.1 -- '*model-runtime*'                              # the test suites
git -C $PI cat-file -e v0.83.0:packages/coding-agent/src/utils/abort.ts                      # fails => new
grep -rnE '^\s*(pub )?(struct|enum|trait|type|impl)\s+ModelRuntime' $CY/crates/ --include='*.rs'
grep -rnoE 'model-registry\.ts:[0-9]+(-[0-9]+)?' $CY/crates/ --include='*.rs' | sort -u | wc -l

# --- DRIFT-040 ---
git -C $PI ls-tree -r --name-only v0.84.1 | grep harness-v2
for t in v0.83.0 v0.84.1; do git -C $PI show $t:packages/agent/docs/harness-v2.md | wc -l; done
git -C $PI show v0.84.1:packages/agent/docs/harness-v2.md | grep -cE '^- \[x\]'   # 10
git -C $PI show v0.84.1:packages/agent/docs/harness-v2.md | grep -cE '^- \[ \]'   # 30
git -C $PI show v0.84.1:packages/agent/docs/harness-v2.md | sed -n '3p;33p;41p;3149p'
git -C $PI show v0.84.1:packages/agent/docs/harness-v2.md | sed -n '1571,1630p;1674,1757p'
for t in v0.83.0 v0.84.1; do git -C $PI show $t:packages/agent/src/harness/agent-harness.ts | wc -l; done
git -C $PI grep -c 'HarnessNotImplemented' v0.84.1 -- 'packages/agent/src'
git -C $PI diff --shortstat -M v0.83.0 v0.84.1 -- packages/storage/ packages/session-backends/
git -C $PI grep -l 'session-backends\|sqlite' v0.84.1 -- 'packages/coding-agent/src'   # empty
git -C $PI show v0.84.1:packages/agent/package.json | sed -n '37,42p'
```
