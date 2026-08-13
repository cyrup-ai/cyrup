# ADR-0004 — pi's agent harness is a published SDK pi does not ship: port the behaviour it pins, not the harness

**Status** accepted (decided by default under the parity rule — overridable)
**Date** 2026-08-13
**Decides** OQ-2 (`PARITY-PLAN.md:1418`)
**Blocks released** Batch 2 (the OQ-2 line item); batch 18's measurement obligation (`PARITY-PLAN.md:879`) — **discharged here, in full, so batch 18 loses that task**; the branch risk at `PARITY-PLAN.md:262` ("if OQ-2 returns *absorb the harness*, every batch after 18 is re-sized") — **the branch does not fire**; and the trustworthiness of the 448-row figure. Trackers `AGENT-028`, `SESS-038`, `DRIFT-040` and `PARITY-GAPS` `VL-P22` are all disposed of below.

---

## Context

### The question

`pi/packages/agent/src/harness/**` is the largest surface in the workspace that **no area file owns**. Four trackers point at it — `AGENT-028` (area 02), `SESS-038` (area 03), `DRIFT-040` (area 12) and `VL-P22` (`PARITY-GAPS`) — and not one proposes work, because each concluded the answer is a scope decision rather than an audit. `PARITY-GAPS.md:834` states the three candidate answers: **(a)** absorb it, **(b)** interop only, **(c)** out of scope until pi's own `coding-agent` migrates.

The plan is explicit that this is the one question where a measurement must precede the answer (`PARITY-PLAN.md:879`: "*Also **measure** `pi packages/agent/src/harness/**` (file count, line count, shape — no port, no items), so OQ-2 is decided against a number*"). That measurement is below. It changed the answer, and it also **refuted the premise the question was written on**.

Every command below was run against `git`-object reads at the named tag (`git show <tag>:<path>`, `git ls-tree <tag>`, `git diff <tagA> <tagB>`). No working tree was modified in either repository.

---

### Measurement 1 — the headline number is wrong, and wrong by 2.3×

Every one of the four trackers repeats the same figure. `PARITY-GAPS.md:654` and `:834`, `PARITY-PLAN.md:1382` and `:1419` all say `packages/agent/src/harness/**` is "**~11.4k insertions / ~10.9k deletions**".

```
$ git -C pi diff --shortstat v0.83.0 v0.84.1 -- packages/agent/src/harness/
 32 files changed, 4977 insertions(+), 2936 deletions(-)
```

The real harness churn is **4,977 / 2,936**. The 11.4k/10.9k figure is the **whole `packages/agent/` package**:

```
$ git -C pi diff --shortstat v0.83.0 v0.84.1 -- packages/agent/
 77 files changed, 11336 insertions(+), 10859 deletions(-)
```

And most of that is not source at all:

```
$ git -C pi diff --shortstat v0.83.0 v0.84.1 -- packages/agent/docs/
  9 files changed, 2700 insertions(+), 5190 deletions(-)
$ git -C pi diff --shortstat v0.83.0 v0.84.1 -- packages/agent/test/
 24 files changed, 3347 insertions(+), 2715 deletions(-)
```

**7,890 of the 11,336 insertions and 7,905 of the 10,859 deletions are docs and tests.** The number that has been driving this question's priority for two passes is a package-level figure misattributed to a subtree. Nothing in the four trackers is dishonest — the figure was carried forward, never re-derived, exactly as `DRIFT-040` itself admits ("*carried forward unverified … and are **still** unverified*", `12-upstream-drift-pi-core.md:116`).

### Measurement 2 — file count, line count, shape at both tags

```
$ git -C pi ls-tree -r --name-only v0.83.0 packages/agent/src/harness/ | wc -l
28
$ git -C pi ls-tree -r --name-only v0.84.1 packages/agent/src/harness/ | wc -l
41
```

Summed `wc -l` over every file at each tag (loop over `git show <tag>:<file>`):

| | v0.83.0 | v0.84.1 |
|---|---|---|
| files | 28 | 41 |
| lines | **7,783** | **9,824** |

So the subtree grew by **2,041 net lines**, from 7.8k to 9.8k. It is a mid-size subsystem, not an 11k-line one.

### Measurement 3 — what it actually is

Thirteen files are new at v0.84.1. By line count:

| new file | lines | what it is |
|---|---|---|
| `session/testing/conformance.ts` | 993 | a **published contract-test kit** for third-party session backends |
| `reducer.ts` | 667 | a pure fold over a durable record log; defines `RecordLogCorruption` + 12 corruption reasons (`reducer.ts:22-34`) |
| `telemetry.ts` | 615 | typed span/attribute **schema declarations** |
| `session/state.ts` | 344 | in-memory session state |
| `session/types.ts` | 372 | the harness-v2 entry/record/lane model |
| `session/jsonl/{storage,codec,repo,types,errors}.ts` | 272/193/179/56/17 | a JSONL codec + repo split out of the old flat `session/jsonl-*.ts` |
| `session/memory.ts` | 192 | in-memory backend |
| `session/context.ts` | 100 | context assembly |
| `session/search.ts` | 71 | search interface |
| `result.ts` | 63 | `Result`/`err`/`ok` helpers |
| `session/{index,jsonl}.ts`, `session/testing/{index,types}.ts` | 13/9/6/16 | barrels |

Two structural facts matter more than the inventory:

1. **The harness is not a test harness.** `packages/agent/src/index.ts` at v0.84.1 re-exports it *wholesale* as the public API of the published npm package `@earendil-works/pi-agent-core` — `export * from "./harness/agent-harness.ts"`, `./harness/result.ts`, `./harness/session/index.ts`, `./harness/session/search.ts`, `./harness/skills.ts`, `./harness/system-prompt.ts`, `./harness/tools/index.ts`, `./harness/messages.ts`, `./harness/prompt-templates.ts`, `./harness/utils/*`, plus named re-exports from `compaction/*`, `telemetry.ts` and `types.ts`. The word "harness" in this path means *the thing that harnesses a model into an agent*, not *a rig for testing*. The assignment's first hypothesis — "internal test/eval harness" — is **false**.

2. **But one part genuinely is a test kit, and it is deliberately published as one.** `packages/agent/package.json:18-21` declares a second export subpath:
   ```json
   "./session/testing": {
       "types": "./dist/harness/session/testing/index.d.ts",
       "import": "./dist/harness/session/testing/index.js"
   }
   ```
   which resolves to `harness/session/testing/index.ts` → `createSessionBackendConformance` (`session/testing/index.ts:1`). This is the "port the conformance expectations" case the assignment anticipated — it is real, but it is 1,015 of the 9,824 lines, not the whole subtree.

### Measurement 4 — the import trace, which is what the answer turns on

**A caution first: `rg "from ['\"].*harness"` — the command the assignment suggested — is poisoned by a name collision and will produce a confidently wrong answer.** `packages/coding-agent/test/suite/harness.ts` is coding-agent's *own* vitest fixture, unrelated to `packages/agent/src/harness/`. It accounts for ~40 of the ~45 hits. Any trace that counts those concludes "coding-agent depends on the harness heavily". It does not. The same trap exists a second time: `coding-agent/src/core/tools/bash.ts:500` exports its own `createBashTool`, so grepping for that symbol also lands on the wrong side (`core/sdk.ts:19-31` imports it from `./tools/index.ts`, **not** from `pi-agent-core`).

The sound trace is by package specifier. Non-test importers of `@earendil-works/pi-agent-core` across the whole monorepo at v0.84.1:

```
$ git -C pi grep -l "@earendil-works/pi-agent-core" v0.84.1 -- 'packages/**/*.ts' \
    | grep -v "/test/" | sed 's|v0.84.1:packages/||;s|/.*||' | sort | uniq -c
  36 coding-agent      (33 in src/, 3 in examples/)
  11 session-backends
```

`ai`, `client`, `evals`, `protocol`, `server`, `telemetry` and `tui` import it **zero** times.

Now take the union of every symbol those 33 `coding-agent/src` files import from `pi-agent-core`, and resolve each to its defining file in `packages/agent/src`:

| symbol | defined at v0.84.1 | inside `harness/`? |
|---|---|---|
| `Agent` | `agent.ts:173` | no |
| `AgentEvent` | `types.ts:428` | no |
| `AgentMessage` | `types.ts:325` | no |
| `AgentState` | `types.ts:333` | no |
| `AgentTool` | `types.ts:386` | no |
| `AgentToolResult` | `types.ts:361` | no |
| `AgentToolUpdateCallback` | `types.ts:383` | no |
| `PrepareNextTurnContext` | `types.ts:147` | no |
| `ThinkingLevel` | `types.ts:300` | no |
| `ToolExecutionMode` | `types.ts:42` | no |
| `StreamFn` | `types.ts:28` | no |
| `setDefaultStreamFn` | `stream-fn.ts:11` | no |
| `AgentHarness` | `harness/agent-harness.ts:305` | **yes** |
| `AgentHarnessOptions` | `harness/agent-harness.ts:243` | **yes** |
| `HarnessTool` | `harness/agent-harness.ts:237` | **yes** |
| `AgentHarnessTool` | `harness/types.ts:81` | **yes** |
| `ExecutionEnv` | `harness/types.ts:315` | **yes** |
| `ExecutionToolContext` | `harness/tools/tool-context.ts:4` | **yes** |
| `createReadTool` | `harness/tools/read.ts:45` | **yes** |
| `createEditTool` | `harness/tools/edit.ts:77` | **yes** |
| `createWriteTool` | `harness/tools/write.ts:15` | **yes** |
| `createBashTool` | `harness/tools/bash.ts:51` | **yes** |

Ten harness-defined symbols are imported. **All ten are imported by exactly one file** — `packages/coding-agent/src/server/create-harness.ts:1-12` — and its import list is a 10-of-10 match:

```
$ git -C pi grep -n "AgentHarness\|HarnessTool\|ExecutionToolContext\|ExecutionEnv" \
    v0.84.1 -- 'packages/coding-agent/src/*' | grep -v "server/create-harness.ts"
<no output>
```

And that one file is **unreachable from anything pi ships**:

```
$ git -C pi grep -n "create-harness\|createCodingAgentHarness" v0.84.1 -- packages/ | grep -v CHANGELOG
  … src/server/create-harness.ts   (its own definitions)
  … test/server/create-harness.test.ts:19-20, :65, :115, :139, :180, :228
```

Nothing in `src/` imports it. `packages/coding-agent/src/server/` contains **only** that one file (`git ls-tree --name-only v0.84.1 packages/coding-agent/src/server/`). And coding-agent's `package.json` publishes only three subpaths — `.`, `./rpc-entry`, `./client` — none of which is `./server`. Its sole consumer is its own unit test.

**What pi's coding agent actually runs is a complete parallel implementation of the same concerns, which cyrup already ported:**

| concern | harness-v2 (unused by the binary) | what pi's binary actually uses |
|---|---|---|
| session store | `harness/session/**` | `coding-agent/src/core/session-manager.ts` (1,714 lines) |
| compaction | `harness/compaction/*` | `coding-agent/src/core/compaction/` |
| tools | `harness/tools/*` | `coding-agent/src/core/tools/*` |
| system prompt | `harness/system-prompt.ts` | `coding-agent/src/core/system-prompt.ts` |
| agent loop | `harness/agent-harness.ts` (`AgentHarness`) | `coding-agent/src/core/agent-session.ts` |

`agent-session.ts:102` imports `SessionEntry`/`SessionManager` from `./session-manager.ts` — its own — never from the harness.

Two further consumers, both dead ends:

- **`packages/session-backends/sqlite-node`** (11 non-test files) implements the harness `SessionRepo` port — `SessionError`, `Entry`, `EntryOrder`, `SessionSearch`, `SessionMetadata`, `SessionCreateOptions`, `FileSystem` — and consumes the conformance kit at `test/conformance.test.ts:4-7`. But it is published as `@earendil-works/pi-session-backend-sqlite-node` (`package.json:2`) and **no package in the monorepo depends on it**: `git grep -n "session-backends\|sqlite-node" v0.84.1 -- 'packages/*/package.json'` returns only sqlite-node's own manifest. It ships to nobody.
- **`harness/telemetry.ts`** (615 lines) has **zero emission sites**:
  ```
  $ git -C pi grep -n "startAiSpan\|startHarnessSpan\|AI_TELEMETRY_SCHEMA\|AGENT_TELEMETRY_SCHEMAS" \
      v0.84.1 -- 'packages/**/*.ts' | grep -v "harness/telemetry.ts\|/test/\|agent/src/index.ts"
  v0.84.1:packages/agent/scripts/generate-telemetry-docs.ts:90: …renderSchema(AI_TELEMETRY_SCHEMA, …)
  ```
  The only reader of the schema is the script that renders it into a markdown table.

**The trace's verdict: harness-v2 is a published SDK rewrite that pi's own product has not adopted. Zero user-visible behaviour of the `pi` binary flows through any line of it.** Both hypotheses in the assignment are false — it is neither a runtime surface the coding agent depends on, nor an internal test harness. It is pi's *next* agent core, shipped to npm ahead of its own migration.

### Measurement 5 — what cyrup has today

cyrup already ports the column pi's binary actually uses, decomposed across crates rather than one `harness/` directory:

| pi (shipping path) | cyrup at HEAD `72cd292` |
|---|---|
| `coding-agent/src/core/session-manager.ts` | `crates/cyrup-session/src/{manager,store,entry,header,ids,layout,listing,migrate}.rs` (`lib.rs:1-14`) |
| `coding-agent/src/core/compaction/` | `crates/cyrup-session/src/compaction/` (12 files) |
| `coding-agent/src/core/system-prompt.ts`, `prompt-templates.ts` | `crates/cyrup-session/src/prompt/` (8 files) |
| `coding-agent/src/core/tools/*` | `crates/cyrup-tools/src/tools/{bash,edit,edit_diff,read,write,find,grep,ls}.rs` (`lib.rs:1-15`) |
| `agent/src/{agent,agent-loop,types,proxy,stream-fn}.ts` | `crates/cyrup-agent/src/{agent,loop_fn,state,event,proxy,stream_fn,hooks,queue,subscriber}.rs` |
| `coding-agent/src/core/agent-session.ts` | `crates/cyrup-session-svc/src/session.rs` |

Note the shape of `crates/cyrup-agent/src`: it contains **no harness module**, and that is correct — it mirrors `packages/agent/src` minus `harness/`, which is exactly the part of that package pi's binary imports.

**So cyrup does already have an equivalent by a different name — of the half that matters.** "Absorb the harness" would not fill a hole; it would add a *second*, unused session/tool/compaction stack alongside the one that runs.

### The false premise inside `VL-P22`

`VL-P22` (`PARITY-GAPS.md:634`) files two defects against cyrup, citing `packages/agent/src/harness/session/jsonl/storage.ts` as upstream. Both evaporate when cyrup is compared against the file it is actually a port of.

**(a) "A torn session-JSONL tail is never repaired."** cyrup's `manager.rs:851-888` skips malformed lines and sets `recovered = true`; the rewrite is gated `if migrated && !recovered`, so a recovered file is never rewritten. That is offered as a bug against harness-v2's atomic repair. But pi's shipping session manager does the same thing cyrup does:

```
packages/coding-agent/src/core/session-manager.ts:306-309
    const entry = JSON.parse(line) as FileEntry;
    …
} catch {
    // Skip malformed lines
```

and again at `:506-508`, with `:560` documenting it as intended (*"Blank and malformed lines are skipped to match loadEntriesFromFile()"*). cyrup's own comment at `manager.rs:857-861` says the same in the same words — *"Skip malformed line, keep the valid prefix … 'last good line wins'"*. **cyrup is at parity.**

**(b) "Fork is not published atomically."** cyrup's `store.rs:86-116` (`create_exclusive`) writes header+entries straight to a freshly created fd. Upstream:

```
packages/coding-agent/src/core/session-manager.ts:1620-1625
    writeFileSync(newSessionFile, `${JSON.stringify(newHeader)}\n`, { flag: "wx" });
    for (const entry of sourceEntries) {
        if (entry.type !== "session") appendFileSync(newSessionFile, …);
    }
```

Non-atomic, exclusive-create, direct to destination — and cyrup's comment at `store.rs:94-97` already records this deliberately: *"`create_new` is the atomic exclusive-create equivalent of Pi's `"wx"` flag … Written directly (no temp+rename) to mirror Pi."* **cyrup is at parity, on purpose, and said so.**

`VL-P22` measured cyrup against harness-v2 — code pi does not run — and called the resulting difference a bug. It is the mirror image of the "port the literal mechanism" failure: here the port is faithful and the *item* drifted to the wrong upstream. This matters beyond one row: `PARITY-GAPS.md:651` calls `VL-P22` "**the single largest ownerless surface in the port**", and `PARITY-GAPS.md:659` schedules its torn-tail half as "*a concrete, small, provable bug [that] should be fixed now*". Fixing it would have made cyrup **less** faithful to pi.

### Claim-by-claim audit of the four trackers

Since `DRIFT-040` flags its own three load-bearing figures as never verified (`12-upstream-drift-pi-core.md:190`), all are re-derived here:

| claim | source | measured | verdict |
|---|---|---|---|
| `agent-harness.ts` `+420/−996` | `DRIFT-040` | `git diff --numstat`: `420  996` | **confirmed exactly** |
| `docs/harness-v2.md` `+2124/−367` | `DRIFT-040` | `2124  367` | **confirmed exactly** |
| sqlite-node rebuild `+12598/−3479` | `DRIFT-040` | `packages/session-backends/`: 37 files, +4,253/−0; with `packages/storage` and rename detection: 49 files, **+4,010/−1,494** | **refuted** — off by ~3×; not reproducible against any subtree |
| `packages/storage` → `packages/session-backends` rename | `DRIFT-040` (confirmed first-hand) | `git ls-tree --name-only v0.83.0 packages/` lists `packages/storage`; v0.84.1 lists `packages/session-backends` | **confirmed** |
| harness/session retree "21 files, +3070/−1147" | area 03 | `git diff --shortstat … harness/session/`: `21 files changed, 3070 insertions(+), 1147 deletions(-)` | **confirmed exactly** |
| harness is "~11.4k/~10.9k" | all four trackers | 4,977 / 2,936 | **refuted** — the figure is `packages/agent/` |

Area 03 and the two `DRIFT-040` claims that were read first-hand are accurate. Every figure that was *carried forward* is wrong. That is the reusable lesson.

---

## Decision

**Port the behaviour the harness pins. Do not port the harness.** Concretely, an implementer does the following and re-derives nothing:

1. **Do not port `harness/agent-harness.ts`, `reducer.ts`, `result.ts`, `harness/session/**`, `harness/telemetry.ts`, or `packages/session-backends/sqlite-node` as structures.** No cyrup crate gains an `AgentHarness`, a record-log reducer, a `SessionRepo` trait, a lane/record model, or a sqlite session backend on account of pi v0.84.1.

   The ground is **not effort** and **not an accepted divergence**. The project rule is 1:1 *behavioural* parity with pi. The trace above proves the behaviour set these files contribute to the `pi` binary is **empty** — 33 `coding-agent/src` files import `pi-agent-core` and not one reaches a harness symbol; the single file that does is on no export path and has no importer; the telemetry layer has no emission site; the sqlite backend has no dependent. There is no behaviour to be 1:1 with. Porting them would add a second, unexercised session/tool/agent stack beside the one cyrup already ships, and *reduce* fidelity to what pi does.

2. **Keep `cyrup-session`, `cyrup-tools` and `cyrup-session-svc` pointed at `packages/coding-agent/src/core/**`.** That is pi's live implementation and the correct upstream counterpart for every session/tool/compaction item in the ledger. **When an item cites `packages/agent/src/harness/…` as the upstream side of a cyrup defect, the citation is wrong** — re-derive it against `coding-agent/src/core/` before filing or fixing.

3. **Close `VL-P22` as refuted on both halves**, per the two-sided reads above (`session-manager.ts:306-309` vs `manager.rs:851-888`; `session-manager.ts:1620-1625` vs `store.rs:86-116`). **Do not implement the torn-tail repair or the atomic fork publish.** Both would introduce divergence from pi's shipping behaviour. Keep the ID with the refutation recorded so the call is re-auditable.

4. **The behaviour genuinely worth taking is the conformance kit's *expectations*, and only where cyrup's format has the same invariant.** `session/testing/conformance.ts` (993 lines) is a published contract suite; most of it asserts harness-v2 concepts cyrup has no counterpart for (records, lanes, operations, provisioned entries). Read it once, and file — as ordinary test items against `cyrup-session` — only the assertions that hold for the coding-agent JSONL format cyrup ships. **Estimate this at a handful of cases, not 993 lines.** File them under area 03 with two-sided citations at v0.84.1. If the honest yield is zero cases, record that result explicitly rather than leaving silence.

5. **Split `AGENT-028` at the package boundary and keep only the live half.** The telemetry question is two questions wearing one ID:
   - `harness/telemetry.ts` — **out**, per (1); zero emission sites.
   - `packages/ai` — **in, and stays as work.** `@earendil-works/pi-telemetry` became a runtime dependency of `packages/ai` at v0.84.1 (`packages/ai/package.json:65`), and `telemetryContext` is on the request options that coding-agent's live path flows through (`packages/ai/src/types.ts:123`, `api/simple-options.ts:36`). cyrup's `stream.rs` `StreamOptions` has no telemetry field. That is `VL-P5` / `DRIFT-047` and is unaffected by this ADR.

6. **Install a tripwire, because this decision is a snapshot of pi's migration state, not a judgement about the code.** Add to whatever upstream-drift sweep runs each version bump a single check:

   ```
   git -C pi grep -l "AgentHarness" <newtag> -- 'packages/coding-agent/src/*' | grep -v server/create-harness.ts
   ```

   **Non-empty output re-opens OQ-2 automatically** and this ADR must be revisited. A second, cheaper signal: any `packages/coding-agent/package.json` dependency on `@earendil-works/pi-session-backend-sqlite-node`. Until one of those fires, harness-v2 is pi's private future and cyrup owes it nothing.

   **Add a third, and watch it first — both of the above are *lagging* signals**, because they fire only once pi has already begun migrating. `docs/adr/LEADS-SETTLED.md` §2.2 found the *leading* one, in upstream's own tracker: `packages/agent/docs/harness-v2.md` §20 carries 40 checkboxed work packages, of which **10 are done and 30 open at v0.84.1** — and zero checkboxes exist at v0.83.0, so this instrument is itself new.

   ```
   git -C pi show <newtag>:packages/agent/docs/harness-v2.md | grep -cE '^- \[ \]'
   ```

   Reaching 0 means the rebuild is finished and pi's calculus about migrating `coding-agent` can change — a signal that arrives *before* either lagging tripwire, not after. Watch all three.

7. **Correct the number wherever it appears.** `packages/agent/src/harness/**` is **4,977 insertions / 2,936 deletions across 32 files** in the v0.83.0→v0.84.1 window; the subtree is **41 files / 9,824 lines** at v0.84.1 and **28 files / 7,783 lines** at v0.83.0. The 11.4k/10.9k figure describes `packages/agent/` and is ~70% docs and tests. It appears at `PARITY-GAPS.md:654`, `:834`, `PARITY-PLAN.md:1382` and `:1419`.

---

## Consequences

### The four trackers

| ID | was | becomes |
|---|---|---|
| `AGENT-028` (`02-cyrup-agent.md:92`, `:133`, `:990`) | tracker · upstream-drift · L · *"pi v0.84.x's typed telemetry contract has no cyrup counterpart"* | **split, then closed.** The harness-telemetry half is **closed — out of scope**, ground recorded: `startHarnessSpan`/`startAiSpan` have zero emission sites outside the doc generator. The live half is **already owned by `VL-P5`/`DRIFT-047`** (`packages/ai`, not `packages/agent`) and needs no new ID. Net: one tracker closes, no item is created. Remains excluded from the severity tally — but now for a *decided* reason rather than a pending one. |
| `SESS-038` (`03-cyrup-session.md:111`, `:124`, `:173`) | tracker · upstream-drift · L · *"`packages/session-backends/sqlite-node` has no cyrup counterpart"* | **closed — out of scope.** Ground: no package in pi's monorepo depends on `@earendil-works/pi-session-backend-sqlite-node`; it ships to no user of `pi`. `03-cyrup-session.md:124` says it "must be answered together with area 02's AGENT-028" — this ADR answers both, together, as required. |
| `DRIFT-040` (`12-upstream-drift-pi-core.md:116`, `:174`, `:190`, `:687`) | tracker · **lead — not yet evidenced** · L · duplicate-of `VL-P22` | **evidenced, then closed.** Its "Lead" row (`:190`) is **discharged**: `agent-harness.ts +420/−996` ✅ and `docs/harness-v2.md +2124/−367` ✅ confirmed exactly; the sqlite-node figure ❌ refuted (true: +4,010/−1,494 with rename detection). Its absorbed `DRIFT-037` residue — *interop with harness-v2-written sessions* — is **closed with it**: nothing writes harness-v2 sessions, so there is nothing to interoperate with. `DRIFT-040` ceases to be a lead. |
| `VL-P22` (`PARITY-GAPS.md:634`, `:651-660`) | *"ownerless and growing"* — the single largest ownerless surface | **refuted on both halves and closed** (Decision §3). It is no longer ownerless, no longer growing, and was never a defect. `PARITY-GAPS.md:651-660` needs rewriting: the three area files that "hand the same mass to it" are handing over a surface that contributes no behaviour. |

**Four trackers close. Zero new items are created by the scope decision itself**, plus at most a handful of small test items from Decision §4.

### Where this ADR meets the others in this batch

- **`SESS-038` is this ADR's to dispose of, and it closes.** `docs/adr/ADR-0008-requirement-ids-and-sdk-surface.md` reaches the same row from OQ-6 and removes it from OQ-6's dependent list on the ground that it was never an SDK-surface question — but ADR-0008 makes no measurement of the sqlite backend's reachability and does not hold it open against this closure. `03-cyrup-session.md:124` required `SESS-038` be answered together with `AGENT-028`; only OQ-2 could do that, and this is where it is done. `SEAM-058`, by contrast, stays a tracker (ADR-0008's reachability rule; trigger re-verified not-fired by ADR-0006).
- **ADR-0008 §C does not pull the harness back in.** Its SDK-parity-by-capability rule ranges over what `packages/coding-agent/src/index.ts` exports — pi's *product* surface — not over `packages/agent`'s published `pi-agent-core` SDK, whose `harness/**` subtree this ADR rules out. The two rules meet exactly at Decision §6's tripwire: a harness symbol appearing on coding-agent's export path is this ADR reopening, not §C widening.
- **`DRIFT-040`'s figures were re-derived independently** in `docs/adr/LEADS-SETTLED.md` §2, which reproduces this ADR's sqlite result (**+4,010/−1,494**, not `+12598/−3479`) from a separate run and adds three grounds this ADR did not have: upstream declares coding-agent migration a **non-goal in writing** (`harness-v2.md:41`, `:3149`), the harness is **25 % built** by its own tracker and throws `HarnessNotImplemented` on the rest, and the compatibility obligation runs **one way**. That agreement is reported, not assumed; LEADS-SETTLED decides nothing on its own.

### Batch by batch

- **Batch 2** — the OQ-2 line item (`PARITY-PLAN.md:242`) is satisfied by this file. Its verification bar ("*nine decisions living in checked-in ADRs … every one of the nine trackers escalated with a named owner or closed with the decision recorded*") is met for OQ-2's four.
- **Batch 18** — its measurement obligation (`PARITY-PLAN.md:879`) is **discharged in full and removed from the batch**. Batch 18 keeps its `8902b4f` closure audit and the six `UW-*` holes, which are unrelated. Its "measure the harness" line should be struck, not re-run.
- **The branch risk at `PARITY-PLAN.md:262` does not fire.** "If OQ-2 returns *absorb the harness*, ~11.4k insertions no area file owns enter scope and every batch after 18 is re-sized." The answer is not "absorb". **No batch after 18 is re-sized. No new area file is created. No new owner is needed** — the surface has no behavioural content to own.
- **The 448 figure is now trustworthy on this axis.** All four trackers were already excluded from the severity tally, so the count does not move — but the reason it does not move is now established rather than pending, which is what `PARITY-GAPS.md:601` and `PARITY-PLAN.md:1420` said was missing.
- **Areas 03 and 12 lose a citation hazard.** Per Decision §2, any future item citing `packages/agent/src/harness/…` against a cyrup session or tool defect is mis-cited by construction. Worth a one-line note in both area files' method sections when the cross-link phase runs.

### One inconsistency this surfaced — root-caused and resolved by convention

The ID for this question is not stable across the workspace. `PARITY-PLAN.md:1418` and `:242` call it **OQ-2**; `PARITY-GAPS.md:658` calls the harness-v2 half **OQ-7**; and `PARITY-PLAN.md:242` glosses **OQ-6** as "SDK-surface parity" while `PARITY-PLAN.md` §7's own OQ-6 entry is the `spec/` question.

**Root cause, established during the cross-link pass:** there are two independent `OQ-N` namespaces. `PARITY-GAPS.md` §6 (`:826-836`) carries its own nine numbered open questions, and its numbers do not match `PARITY-PLAN.md` §7's. The mapping is `PG §6 q3 = OQ-5` · `q4 ⊂ OQ-6` · `q6 = OQ-9` · **`q7 = OQ-2`** (this ADR) · `q8 = OQ-3` · `q9 = OQ-1`. Three other ADRs hit the same collision independently — ADR-0003 on q9, ADR-0005 on q8, ADR-0011 on q6 — which is how it was found and why it is a convention rather than four local footnotes.

**Convention, now binding:** an unqualified `OQ-N` means `PARITY-PLAN.md` §7; `PARITY-GAPS`' list is always cited as `PARITY-GAPS §6 q<N>`. It is recorded once, in `docs/adr/README.md`. This ADR decides **OQ-2, which is `PARITY-GAPS` §6 q7**.

---

## Rejected alternatives

**(a) Absorb the harness** — port `AgentHarness`, `reducer.ts`, `harness/session/**`, `telemetry.ts` and a sqlite backend into cyrup, with a new area file and an owner.
*Cost:* ~9,800 lines of Rust plus a sqlite backend (~4,000 more upstream lines) delivering **zero** behavioural change to the cyrup binary, since the trace shows no pi code path reaches any of it. It would create a second session store, a second tool set and a second compaction path alongside the ones `cyrup-session`/`cyrup-tools` already ship — and then require every future session item to decide which of two stacks it targets. `PARITY-PLAN.md:262` prices the schedule damage: every batch after 18 re-sized. Worst of all, it makes cyrup *less* like pi: cyrup would run an agent core pi itself does not run. Rejected on fidelity, not on size.

**(b) Interop only** — keep writing the coding-agent format, but be able to *read* harness-v2-written sessions (`PARITY-GAPS.md:834` option (b), and the `DRIFT-037` residue folded into `DRIFT-040`).
*Cost:* the read path would be dead code the day it merged. Nothing writes harness-v2 sessions: `AgentHarness` is instantiated only at `coding-agent/src/server/create-harness.ts:151`, which no shipping file imports, and the sqlite backend that would persist them has no dependent. Cyrup would carry a decoder for a format no `pi` installation produces — an untestable surface that rots until the tripwire in Decision §6 makes it necessary anyway, at which point it would be rewritten against whatever pi actually shipped. Rejected as speculative.

**(c) Defer again — "decide after batch 18's measurement"** (`PARITY-PLAN.md:1421`).
*Cost:* this is what the plan already scheduled, and it is precisely the failure mode batch 2 exists to end. The measurement is cheap — every command in this ADR runs in seconds against `git` objects. Deferring it a third pass would have carried the wrong 11.4k figure into a fourth, and would have left `VL-P22`'s torn-tail "fix" scheduled as "*should be fixed now*" (`PARITY-GAPS.md:659`) — a change that would have **introduced** a divergence from pi. Deferral had a concrete cost, and it had already been paid twice.

**(d) Port only the 993-line conformance suite wholesale**, as the assignment's second hypothesis suggests.
*Cost:* the suite asserts against the harness-v2 `SessionRepo` interface — records, lanes, operations, provisioned entries, and the twelve `RecordLogCorruption` reasons at `reducer.ts:22-34`. cyrup has none of those concepts, because pi's shipping session manager has none of them either. Porting it wholesale means first porting the model it tests, which is alternative (a) by another route. Decision §4 keeps the part that survives the format difference and says so explicitly, including the instruction to record a zero result rather than leave silence.

---

## How to reverse this

> *"Port pi's harness-v2 — I want cyrup's agent core to match where pi is going, not only where pi is today."*

That sentence overturns it, and it is a legitimate thing to want: it changes the target from *behavioural parity with the `pi` binary* to *structural parity with the `pi-agent-core` SDK*, which is the broader question `PARITY-GAPS.md:831` raises across four area files. What would have to change: a new area file owns `packages/agent/src/harness/**` with a named owner; `AGENT-028`, `SESS-038`, `DRIFT-040` and `VL-P22` reopen as work rather than closing; ~9,800 lines enter scope with `session-backends/sqlite-node` behind them; and every batch after 18 is re-sized per `PARITY-PLAN.md:262`.

It also reverses **automatically, without anyone saying anything**, the moment the tripwire in Decision §6 fires — i.e. when pi's own `coding-agent/src` imports `AgentHarness` outside `server/create-harness.ts`. That is the event this decision is really waiting on, and pi will announce it in code.
