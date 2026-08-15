# 00 — Residual ledger

Ranked, cross-cutting view. The per-area files hold the evidence; this file is for **picking the
next work item**.

---

# RECONCILED 2026-08-14 (second edition) — six sweeps applied, every count re-derived from the twelve area tables

> **cyrup HEAD `bdcb0d0`** (was `380c713` when the first edition below was written), branch
> `david/cyrup`, tree clean. Gate: **`cargo nextest run --workspace` = 6699 tests, 6699 passed,
> 7 skipped, 16.3 s**; `cargo check --workspace --all-targets` clean.
>
> **Everything below this block — including the `RECONCILED 2026-08-14 — two parity sweeps applied`
> header that follows it — is the previous edition and is superseded on every number.** It is
> retained unedited because its *reasoning* is still the argument this file rests on. Only the
> arithmetic is dead. Where a sentence in it is wrong about the CODE or about UPSTREAM (not merely
> out of date), it has been struck in place; those strikes are listed under
> "Corrections to the previous edition" below.

## What happened: sweeps 3, 4, 5 and 6

The first edition reconciled sweeps 1-2. **Four more sweeps ran before any doc writer did**, and that
delay is itself the largest finding of this edition — see "Why sweep 6 'refuted' 39 rows" below.

| sweep | shape | landed | note |
|---|---|---|---|
| 1 | per-crate, 11 agents | 232 items | 184 `docUpdatesNeeded`; reconciled in the first edition |
| 2 | per-crate | (see first edition) | reconciled in the first edition |
| 3 | per-crate | — | **never reconciled into these files** |
| 4 | per-crate | **15 items only** | the stall; see the orchestration finding |
| 5 | **by FEATURE** | **5 of 5 assigned, incl. `SUBA-S01`** | the unblock |
| 6 | **by FEATURE**, 5 agents | **~15 landed, ~39 rows refuted** | reconciled here |

## The open set — derived from the twelve `## Open items` tables, nothing carried forward

| | critical | high | medium | low | **counted open** |
|---|---:|---:|---:|---:|---:|
| first edition (after sweeps 1-2) | 0 | 3 | 75 | 95 | **173** |
| **this edition (after sweeps 3-6)** | **0** | **2** | **63** | **88** | **153** |

**338 of 500 rows across the twelve tables now carry a closure marker** (was 311 of 492). **Ten
`tracker` rows are excluded from every figure**, as always — eight in the area tables (`PERM-017`
was re-classified this edition; its own body says "No action while the middle levels remain
meaningless", which is the Trackers contract) plus `SEAM-058` and `SUBA-005` in their areas'
separate `## Trackers` tables.

**Eight rows are new since the first edition, and FOUR of them were filed AND closed in the same
pass** — which is what a *hunting* sweep produces when it finds a defect the backlog never named:
`TOOL-042` (the nextest `LEAK`, root-caused, area 04), `EXT-M01` and `EXT-M02` (two JS→Rust mechanism
gaps, area 06) and `PERM-033` (16 unported forwarding audit sites, area 10). One more was filed and
**partially** closed: `TUI-062` (area 07, still counted open). Three were filed open: `CFG-052`
(`github:` shorthand), `CFG-053` (test debt re-filed out of `CFG-006`) and `ICOM-053` (re-filed out
of `ICOM-026`'s structural half).

### By area

| file | crit | high | medium | low | **open** | Δ vs first edition |
|---|---:|---:|---:|---:|---:|---|
| `01-cyrup-core-and-provider.md` | 0 | 1 | 4 | 6 | **11** | −1 (`PROV-011`) |
| `02-cyrup-agent.md` | 0 | 0 | 0 | 2 | **2** | — |
| `03-cyrup-session.md` | 0 | 1 | 1 | 6 | **8** | — |
| `04-cyrup-tools.md` | 0 | 0 | 1 | 4 | **5** | −1 (`TOOL-016`; `TOOL-042` filed+closed) |
| `05-cyrup-config-and-resources.md` | 0 | 0 | 5 | 7 | **12** | −6 (8 closed, 2 filed) |
| `06-cyrup-ext.md` | 0 | 0 | 11 | 13 | **24** | — (`EXT-M01`/`EXT-M02` filed+closed) |
| `07-cyrup-tui.md` | 0 | 0 | 14 | 21 | **35** | +1 (`TUI-062` filed) |
| `08-cyrup-session-svc-and-modes.md` | 0 | 0 | 3 | 4 | **7** | −1 (`SEAM-061`) |
| `09-cyrup-ext-subagents.md` | 0 | 0 | 10 | 10 | **20** | −6 |
| `10-cyrup-permission-system.md` | 0 | 0 | 2 | 2 | **4** | −1 (`PERM-017` → tracker; `PERM-033` filed+closed) |
| `11-cyrup-intercom.md` | 0 | 0 | 5 | 4 | **9** | −5 |
| `12-upstream-drift-pi-core.md` | 0 | 0 | 7 | 9 | **16** | — (no sweep-6 agent owned it) |
| | **0** | **2** | **63** | **88** | **153** | **−20** |

**The two remaining highs — the whole actionable high set:**

| # | ID | area | why it is still the top of the backlog |
|---|---|---|---|
| 1 | **`SESS-040`** | 03 + 07 + 08 | A shipped control that bills tokens and rewrites the session file still has no dispatch site, and the UI advertises it. Its two siblings (`SESS-041`, `SESS-042`) are closed, so **the moment `SESS-040` lands a caller the abort actually takes effect.** |
| 2 | **`PROV-047`** | 01 + 02 + 06 + 08 | `httpProxy` reaches OAuth and the ADC minting path, but **the fix stays inert in production until one line lands**: `configure_http_proxy(...)` beside the existing `configure_http_idle_timeout(timeout_ms)` in `cyrup-session-svc/src/builder.rs`. Two further one-liners (`cyrup-agent/src/proxy.rs:468`, `cyrup-ext/src/caps/http.rs:599`) complete it. |

`SEAM-061` — ranked #1 in the first edition — **is closed as REFUTED**: sweep 6 found it already
landed at HEAD in *both* crates (`cyrup-tui/src/session_selector.rs:154`/`:276`/`:313`/`:1918`/`:1985`
and `crates/cyrup/src/main.rs:1354` + `startup_ui.rs:191-201`). **Areas 08, 09 and 10 now have zero
open criticals and zero open highs between them.**

## Why sweep 6 "refuted" 39 rows — two different failures wearing one word

**This is the correction a reader most needs, and it cuts the other way from the first edition's
headline.** Sweep 6 recorded ~39 `refuted-not-fixed` outcomes across five agents. They are **not** 39
analysis errors. Separate them:

- **(b) DOC STALENESS — roughly 32 of the 39.** The analysis was right, a sweep between 3 and 5
  landed the fix, and **no writer ever reconciled it**, so the row still said "still open". Area 06
  alone accounts for eighteen: every one of `EXT-007`, `-009`, `-011`, `-016`, `-018`, `-023`, `-030`,
  `-032`, `-033`, `-034`, `-036`, `-044`, `-045`, `-046`, `-049`, `-052`, `-056`, `-057` still read
  "still open" in `## Status of every item from prior analyses` **while that file's own
  `## Open items` table had already marked them CLOSED.** Two tables in one file disagreed for four
  sweeps. Areas 09 and 11 each contributed six more of the same kind.
- **(a) GENUINE ANALYSIS ERROR — roughly 7.** Fabricated or wrong citations (`PROV-011`'s claim that
  pi's built-in Edit/Write/Read/Bash declare `constrainedSampling` — **no pi built-in does, three
  grep hits total at v0.83.0**; three fabrications and four wrong-offset clusters in `EXT-036`),
  a wrong premise (`LEAK-FAIL`'s "the victim is arbitrary" — nextest runs each test in its own
  process, so the victim can only be the test that spawned the leak), a misclassification
  (`PERM-017` is a tracker, not work), stale headline evidence (`CFG-014`'s "grep returns ZERO",
  `CFG-048`'s "~30-name rename table" — it is **59**, diffed pair-for-pair), and **one refutation
  that was itself wrong** (`PERM-008-R2`, below).

**Combined across all six sweeps: ≈53 recorded refutations against ~430 items worked ≈ 12%** — the
same rate the first edition measured, and it did not improve. **But the failure mode has shifted:**
sweeps 1-2's refutations were mostly type (a); sweep 6's are mostly type (b). **Type (b) is cheaper
per instance and far more expensive in aggregate, because it costs a whole agent-pass to rediscover
and it is entirely preventable by reconciling documentation every sweep instead of every four.**

**One refutation was itself wrong, and this is the sharpest lesson in the file.** Sweep 2 refuted a
third of `PERM-008`'s Verify recipe on the ground that pi emits no warning for a malformed forwarded
request: `if (!request) { safeDeleteFile(...); continue; }` at `index.ts:1144-1147` has no log call
above it. **The reading of the CALLER is correct; the conclusion is not** — upstream logs one frame
down, inside `readForwardedPermissionRequest` (`:942` catch, `:928` field-ladder). The test that
resulted **pinned cyrup-invented silence as if it were parity**, and sweep 6 had to un-pin it.
**A test that pins ABSENCE is only as good as the frame depth of the citation behind it. Absence
claims require the callee to be opened too.**

## ORCHESTRATION — per-crate partitioning stalled at sweep 4; partitioning by FEATURE unblocked it

**Record this for whoever runs sweep 7; it is the highest-leverage finding of this edition and it is
about the process, not the code.**

- **Sweep 4, partitioned per-crate, landed 15 items.** Per-crate ownership strands everything
  cross-cutting: an agent that owns the crate where the defect is *observed* usually does not own the
  crate where the fix *lands*, so the honest outcome is "blocked", every pass, forever.
- **Sweep 5 repartitioned by FEATURE — each agent owning every crate its feature needs — and landed
  all five assigned items, including `SUBA-S01`,** which three per-crate passes had left blocked.
- **Sweep 6 kept the feature partition and landed ~15**, including `PROV-011`, which had been
  reported "clean" by five consecutive provider-side re-verifications **because both of its two
  remaining defects were plumbing frames in the middle** (`cyrup-agent/src/agent.rs:818` and
  `cyrup-ext/src/wrapper.rs`) — sites no per-crate provider agent would ever have opened.

**Three limits of the feature partition, all observed in sweep 6, all worth designing around:**

1. **Ownership partitions the WRITE set; it says nothing about the BUILD set,** which is the
   dependency closure. `crates/cyrup-intercom` cannot type-check while `crates/cyrup-ext` is
   mid-edit, and cyrup-ext's WIT was being changed by a concurrent agent for ~20 minutes of the pass.
2. **Exclusive crate ownership was violated twice, both times for good reason.** The provider agent
   edited `crates/cyrup-ext/src/wrapper.rs` (one delegating override) because landing only its own
   half would have shipped a feature no tool could use — the deferral pattern the standing rules
   forbid. `crates/cyrup/src/main.rs` was edited by two agents in the same window, one of them via
   **whole-file scripted rewrites** — a clobbering risk that agent itself flagged and said it would
   not repeat. **A shared bin like `crates/cyrup/src/main.rs` should be assigned to exactly one agent
   per sweep, or edited only through anchored patches.**
3. **A "feature" must actually be one.** Area 04's tail was paired with area 11's, and they have
   nothing in common: **not one open row in `04-cyrup-tools.md` has a fix site inside
   `crates/cyrup-tools/**` any more** (`TOOL-015`/`-022` → cyrup-tui + cyrup-core, `TOOL-017` →
   cyrup-tui + a product decision, `TOOL-024` → cyrup-ext, `TOOL-031`'s residual →
   cyrup-ext-subagents). **Area 04 is finished as a crate; what remains is five rows filed under the
   wrong area.** Every affected row now names its FIX SITE, and `07-cyrup-tui.md` carries a routing
   table of the eleven foreign-filed rows that land in it. **Route sweep 7 by fix site, not by area
   number** — or it will again spawn an agent with no reachable work.

## Test architecture and the gate — updated numbers

- The integration tests were relocated into their crates as unit tests (`63d729a` / `c3982b5` /
  `d973906`): **310 integration binaries → 6 + 8 gated**, behind the `cyrup-it` harness crate.
- **The gate is now 6699 tests in 16.3 s** (was 6440 in 16.4 s at the first edition; 3932 and 6387
  are older still). **Every `crates/<crate>/tests/<x>.rs` citation in this directory is stale unless
  it names `cyrup-it`.**
- **Structural defect J is unchanged and now has a second and third instance.** `crates/cyrup-it` is
  `required-features = ["it"]`, so the 16-second gate builds and runs **none** of it. Consequences
  now on the record: (i) `EXT-025` cannot be closed by deleting its dead methods, because the
  breakage would land silently in the un-built crate — it needs **one** agent owning cyrup-ext +
  cyrup-session-svc + cyrup-it in a single commit; (ii) the 0.6 and 0.7 `HOST_WORLD` bumps have
  **never been instantiated against a real guest**, because that fixture lives there too, and the
  failure mode is an opaque wasmtime LINK error; (iii) the four `cyrup-it` assertions the first
  edition cited as contradicting production **no longer do** — sweep 6 re-read them and they match
  (`ICOM-026`, closed as refuted). The gap is now filed as **`ICOM-053`** so it is tracked as a
  statement about the GATE rather than hidden inside a closed test-defect row.

## Corrections to the previous edition — wrong about the CODE or about UPSTREAM, not merely stale

1. **`CFG-042`/`CFG-048`'s "needs `indexmap` in the workspace dependency table" is VOID.** The
   insertion-ordered map landed in sweep 6 as a ~60-line local `OrderedObject` in
   `crates/cyrup-config/src/models_store.rs` with **no new dependency**. The mechanism claim in
   register entry **D** is right; its prescription was wrong.
2. **`PERM-008`'s "REFUTED ONE THIRD OF THE RECIPE" is struck** — see above. The observation about the
   caller stands; the conclusion and the test it produced were wrong.
3. **`SEAM-061`'s ranking note ("two sweeps have split this across areas 07 and 08 and neither took
   it") is struck** — it was already closed at HEAD in both crates.
4. **The claim that pi's built-in tools declare `constrainedSampling` is struck.** Three grep hits
   exist at v0.83.0, all of them the `ToolDefinition` field (`extensions/types.ts:463`) and the two
   `tool-definition-wrapper.ts` copies (`:14`, `:42`). **Adding opt-ins to cyrup-tools' built-ins
   would be a divergence FROM pi.**
5. **`SUBA-N03`'s closure is wider than recorded**: it covers **eleven** `async:true` single-mode
   parameters, not eight, and the load-bearing half — that the parameters reach the **detached hop-2
   runner** at the `runner-config.json` boundary, not merely past the router — is separately pinned.


---

# RECONCILED 2026-08-14 — two parity sweeps applied, every count re-derived

> **cyrup HEAD `380c713`** (was `04c1ba2` when the edition below was written), branch `david/cyrup`,
> tree clean. Gate: **`cargo nextest run --workspace` = 6440 tests, 6440 passed, 8 skipped, 16.4 s**;
> `cargo check --workspace --all-targets` clean.
>
> **Everything below this block — including the `REGENERATED 2026-08-12` header that follows it — is
> the previous edition, and is superseded on every number.** It is
> retained unedited because its *reasoning* — the severity-scale correction, the duplication census,
> the structural-defect list — is still the argument this file rests on. Only the arithmetic is dead.

## What happened, honestly

Two whole-backlog parity sweeps ran against this directory's twelve area files. Area agents were
**forbidden from editing documentation** so that a single writer could reconcile all sixteen files in
one pass without sixteen partial edits racing each other; this section, and the per-row dispositions
now written into every `## Open items` table, are that reconciliation.

**Sweep 1** landed **232 items across 11 crates** (commit `380c713`) and handed back **184
`docUpdatesNeeded` entries**. **Sweep 2** ran the same shape over what remained. Both were restricted
to `cargo check -p <crate> [--all-targets]` — **neither executed the test suite**; the orchestrator
ran the gate once over the combined work.

### The open set — before and after, derived from the twelve tables, nothing carried forward

| | critical | high | medium | low | **counted open** |
|---|---:|---:|---:|---:|---:|
| **before** (the twelve tables as this reconciliation found them) | 1 | 34 | 195 | 233 | **463** |
| **after** | **0** | **3** | **75** | **95** | **173** |

**290 rows moved to closed.** 21 rows already carried a closure marker before either sweep, so
**311 of the 492 rows across the twelve tables now carry one**. Eight rows are new: seven filed and
closed in the same pass (`PROV-053`, `AGENT-034`, `AGENT-035`, `SESS-045`…`SESS-048`) and one filed
open (`EXT-060`). **Nine `tracker` rows** are excluded from every figure above, as always — seven in
the area tables, plus `SEAM-058` and `SUBA-005` in their areas' separate `## Trackers` tables.

Note that the "before" row does not match this file's own previous headline of 448 raw / ~420
distinct, nor `README.md`'s 458. Both were computed before the repair pass finished growing the
tables, and neither was re-derived afterwards. **463 is what the tables actually contained**; every
figure in this section was produced by parsing them, and the parse is reproducible.

### By area

| file | crit | high | medium | low | **open** |
|---|---:|---:|---:|---:|---:|
| `01-cyrup-core-and-provider.md` | 0 | 1 | 5 | 6 | **12** |
| `02-cyrup-agent.md` | 0 | 0 | 0 | 2 | **2** |
| `03-cyrup-session.md` | 0 | 1 | 1 | 6 | **8** |
| `04-cyrup-tools.md` | 0 | 0 | 1 | 5 | **6** |
| `05-cyrup-config-and-resources.md` | 0 | 0 | 11 | 7 | **18** |
| `06-cyrup-ext.md` | 0 | 0 | 11 | 13 | **24** |
| `07-cyrup-tui.md` | 0 | 0 | 14 | 20 | **34** |
| `08-cyrup-session-svc-and-modes.md` | 0 | 1 | 3 | 4 | **8** |
| `09-cyrup-ext-subagents.md` | 0 | 0 | 14 | 12 | **26** |
| `10-cyrup-permission-system.md` | 0 | 0 | 2 | 3 | **5** |
| `11-cyrup-intercom.md` | 0 | 0 | 6 | 8 | **14** |
| `12-upstream-drift-pi-core.md` | 0 | 0 | 7 | 9 | **16** |
| | **0** | **3** | **75** | **95** | **173** |

**The three remaining highs, and they are the whole actionable set:**

| # | ID | area | why it is still the top of the backlog |
|---|---|---|---|
| ~~1~~ | ~~**`SEAM-061`**~~ **CLOSED 2026-08-14 — REFUTED (sweep 6): already landed at HEAD in BOTH crates — `cyrup-tui/src/session_selector.rs:154`/`:276`/`:313`/`:1918`/`:1985` and `crates/cyrup/src/main.rs:1354` + `startup_ui.rs:191-201`. The "blocking evidence" below, and the "two sweeps split this and neither took it" note, are both wrong at HEAD.** | ~~08 + 07~~ | The `--resume` picker lists every project's sessions under a header that says "Current Folder", with a `tab scope` hint bound to nothing. The blocking evidence has NARROWED to one crate: `SessionScope`, `SessionSelector::set_scope` and `scope()` all exist (`session_selector.rs:54`, `:250`, `:255`); what is missing is `SessionAction::ToggleScope` in `keymap.rs:888-909`, its `handle` arm, and making `show_path` follow the scope. **Two sweeps have split this across areas 07 and 08 and neither took it. It must go to one agent holding both crates.** |
| 2 | **`SESS-040`** | 03 + 07 + 08 | A shipped control that bills tokens and rewrites the session file still has no dispatch site. Its two siblings are now closed — `SESS-041` (auto-compaction token) and `SESS-042` (the `aborted: true` payload) — so **040, 041 and 042 now differ only in wiring: the moment 040 lands a caller the abort actually takes effect.** Blocked on `TUI-055`'s consequence, not on `TUI-055`: the band renders now, but nobody has watched it. |
| 3 | **`PROV-047`** | 01 + 02 + 06 + 08 | `httpProxy` now reaches OAuth and the ADC minting path, but **the fix is inert in production until one line lands**: `configure_http_proxy(...)` beside the existing `configure_http_idle_timeout(timeout_ms)` in `cyrup-session-svc/src/builder.rs`. Two further one-liners (`cyrup-agent/src/proxy.rs:468`, `cyrup-ext/src/caps/http.rs:599`) complete it. |

## The analysis's own error rate is now measured, not estimated

This is the most useful thing the two sweeps produced, and it is a property of *this directory*, not
of the port.

- **Sweep 1 refuted 31 items** out of ~290 it worked — **≈11%** — including 9 of 23 in
  `cyrup-tools` and 8 of 41 in `cyrup-session-svc`.
- **Sweep 2 recorded 16 further `refuted-not-fixed` outcomes** and roughly a dozen more in-body
  factual corrections to items it *did* fix (a wrong upstream constant, a wrong line range, a wrong
  premise about what blocks the work).
- Combined: **≈47 recorded refutations against ~380 items worked across the two sweeps ≈ 12%.**
  **Fifteen rows carry an explicit `REFUTED, CLOSED 2026-08-14` marker**; the rest are recorded
  inside the closure note of an item that was also fixed, or as a correction on a row that stays
  open. No refuted ID was renumbered or deleted.

**Refuting is a success, not a shortfall** — but a 12% error rate has three consequences a planner
must act on:

1. **A stale-closed item costs a whole pass.** `PROV-027`, `PROV-028` and `PROV-029` — three of area
   01's six highs — were already fixed and were closed by *neither* sweep's code work; the file had
   simply never been re-read. `SEAM-047`, `SEAM-051`, `SEAM-064`, `SEAM-072` and `DRIFT-049` were all
   marked fixed in their *kind* cell while their *severity* cell still read `high`, which is how two
   consecutive recounts published phantom highs.
2. **Verify a deferral's stated blocker before accepting the deferral.** `PROV-030` sat open through
   sweep 1 on the ground that "cyrup-provider has no crypto/JWT dep". `ring` 0.17 was already in
   `Cargo.lock` via rustls — checkable in thirty seconds. The same deferral was masking a second,
   independent defect (`PROV-053`) that would have kept the feature dead anyway. Likewise
   `EXT-007`'s "blocked on the `Tool::prompt_guidelines` signature", which three separate places in
   the record were still planning around after the signature had been widened.
3. **A handoff field is not a record.** `SESS-037` appears in none of sweep 1's structured
   `fixedIds` / `partial` / `notReached` buckets because its area's free-text `unresolved` string was
   **truncated at exactly 2500 characters mid-sentence**. It was re-verified from scratch and would
   have been re-worked. That truncation is systematic across areas in the handoff file. Area 01's
   sweep-2 agent independently reported the same thing: its structured fields said "1 not-reached, 2
   partial" while the prose named six not-reached and mentioned none of the three highs it found
   already closed. **Plan from the prose, not the buckets — or fix the buckets.**

## JS→Rust mechanism gaps — the register

This is the highest-value class in this port, and it is not "cyrup does X where pi does Y". It is
**pi relying on a guarantee Rust does not give**. Sweep 1's dedup deadlock came from this class; so
did the two worst near-misses of sweep 2. Every entry below is recorded on the item it belongs to;
they are collected here because the *class* is what a future pass should hunt, not the instances.

**A. A future can be dropped at any `.await`; a JS `async` function always settles.**

- Sweep 1's dedup deadlock: a `PendingOwner` died at an `.await` without settling, and its cache
  entry's live `Sender` kept the channel open forever. pi cannot reach it.
- `DRIFT-029`: pi's `executeBash` removes its `AbortController` in a `finally`. A plain
  `let … = …; …; remove()` sequence leaks a live `CancelToken` on a dropped future and makes
  `is_bash_running()` **permanently true**. Closed with an RAII `BashCancelGuard`.
- `EXT-034`: a re-entrancy latch cleared only on the success path stays set forever after an aborted
  run and permanently disables bus delivery. `bus::DrainLatch` is therefore RAII, not a
  `store(false)` at the end.
- `AGENT-035`: a cancelled Rust stream can end **cleanly** where a JS read loop always throws, so a
  cancel landing after the last `None` was invisible and the consumer saw a well-formed stream end
  with no terminal event.
- **Generalisation, cheap to run:** `grep -n 'finally {' packages/coding-agent/src/core/agent-session.ts`
  enumerates every pi site where a `finally` mutates session state. Each one is a place the Rust port
  needs either a guard or a provably-reached statement.

**B. A `tokio::Mutex` has no deadlock detection, so the failure mode is a HANG, not a red test.**

`EXT-034`'s first cut drained the bus unconditionally at every dispatch seam. `dispatch_*_excluding`
with `exclude = Some(id)` exists precisely because that guest is **suspended inside its own
`provider-stream.on-payload` host import, holding its single-instance store guard** — so a queued
event addressed to it would have re-taken a lock it already held: an infinite await. pi cannot reach
it: its runner is one JS process, `emit` runs listeners synchronously, and a re-entered handler is an
ordinary nested call with no lock. **The invariant — never drain while a dispatch carries an
`exclude` — is now a comment, a test, and a line in `EXT-034`'s row, because removing the argument
reintroduces a hang rather than a failure.**

**C. TypeScript field types are compile-time only; pi's readers re-validate by hand.**

`SESS-045`: `interface SessionHeader` declares `timestamp` and `cwd` required, but pi's two runtime
validators check `type` and `id` and nothing else, and every consumer carries a
`typeof … ? … : fallback` ternary. Rust's serde turns the declared type into an **enforced** one, so
cyrup hard-failed the whole session file where pi opens it and falls back to `process.cwd()`.
**Anywhere cyrup mirrored a pi `interface` field-for-field as a required Rust field, check whether
pi's readers re-validate it.** `SESS-001` and `SESS-027` are the same mechanism one layer down.

**D. JS coercion has no Rust counterpart, in four distinct shapes.**

- `SESS-048`: `typeof x === "number"` is true for negatives and fractions, so pi's `delete` runs for
  every number where an `as_u64` match returns early. **The Rust idiom for a JS number guard is
  `as_f64`.** And `entries[2.0]` is `entries["2"]` — a JS array index is stringified, so pi resolves
  an integral float where cyrup resolved nothing.
- `SESS-046`: `Array.prototype.sort()` with no comparator is **UTF-16 code-unit order**, which is not
  Rust's `Ord for str` (UTF-8 byte order). They disagree for every pair where one string carries an
  astral code point and the other a code point in U+E000..=U+FFFF. **Any `BTreeSet`/`.sort()` that
  reproduces a pi `.sort()` over user-supplied strings — file paths, model ids, session names, skill
  names — is a candidate.** This one reached persisted summary text.
- `PROV-053`: `std::env::var(name).ok()` returns `Some("")` for a blank variable where pi's ambient
  accessor collapses it to `undefined`. Every precedence chain ported from a JS `??` then reads a
  blank var as CONFIGURED — **a silent inversion of the upstream semantics at every such site, and
  the bug is in the adapter, not in any call site, so a per-item review of the call sites cannot see
  it.**
- `CFG-042` / `CFG-048`: `serde_json::Map` is a `BTreeMap` in this workspace, so it cannot represent
  the insertion order a JS object literal carries and `JSON.stringify` writes. A straight port of
  `orderKeybindingsConfig` would have **silently alphabetised the user's keybindings file**. Handled
  with `Vec<(String, Value)>` plus a hand-written two-space printer; the same gap blocks `CFG-042`'s
  remaining half and needs `indexmap` in the workspace dependency table.

**E. pi keys maps on object identity; Rust has none for a value type.**

`PROV-035`: `collectCacheMisses` returns `Map<AssistantMessage, CacheMiss>` keyed on the message
**reference**, and its consumer looks it up by the same reference while re-rendering. Two structurally
equal `AssistantMessage`s are the same key in Rust. Resolved by keying on the entry index, which
pushes an obligation onto the renderer. `DRIFT-029` is the same shape: pi's Set keys on
`AbortController` identity, **not** on the optional-and-repeatable `options.id` the item proposed.

**F. Rust ports keep inventing defensive clamps upstream does not have.**

`SESS-047`'s `.max(1)` on the summarization `maxTokens` had no counterpart in pi's
`Math.min(Math.floor(frac * reserveTokens), …)`. This is the *inverse* of the usual gap — not a JS
guarantee Rust lacks, but a Rust-side clamp invented to avoid a zero pi is perfectly happy to send.

**G. A faithful port would sometimes reproduce an upstream bug, and that decision needs recording.**

pi's `generateId(byId)` inside `migrateV1ToV2` guards against a set the function never `.add()`s to,
so **pi's collision check is a no-op and can mint the same 8-hex id twice**. cyrup's working dedup
can only differ in the case pi gets wrong. Recorded in-source rather than ported. The same question
will recur.

**H. Two cyrup-original surfaces are wearing fabricated pi citations.**

`EXT-036` was filed for this class in the event catalog. Sweep 2 found a second instance in
`interface ui`: `working-start` / `working-stop` are documented "(Pi startWorking/stopWorking,
types.ts:265-275)" and **neither function exists upstream at v0.83.0** — the cited range is
getAllThemes/getTheme/setTheme/getToolsExpanded. Worse, the invented shape is strictly weaker than
pi's: it welds the message to the visibility, so `setWorkingVisible(false)` with the message intact
was unexpressible. `EXT-060` is the third instance, filed by this reconciliation: cyrup's
`registry.tool_info()` emits a `source: "extension"|"guest"` discriminator that pi's `ToolInfo` does
not have — **the WASM-vs-native tier leaking into a guest-facing introspection API**, a distinction
pi's one-extension-kind model has no word for. **Re-run the `EXT-036` sweep over the import surfaces,
not only the event catalog.**


**I. Instance-scoped host state substituted for a call-scoped pi parameter must be torn down in `Drop`. (`EXT-M01` — the sixth real bug from this class.)**

`LiveExtension::execute_tool` binds the tool's `CancelToken` on `GuestState` before awaiting the guest
call and cleared it on both arms of a `tokio::select!`. Upstream's `signal` is a **parameter** of
`ToolDefinition.execute` (`extensions/types.ts:483` @v0.83.0), so it is call-scoped by the language and
a started `async` function always settles. A Rust future has a **third** exit neither `select!` arm
covers — being dropped at the await by an outer `select!`, a `timeout`, or an aborted `JoinHandle` —
and on that path the cancelled token stayed bound forever. `host-tool.is-cancelled` is **not gated to
tool calls** (`world.wit:773`), so every later poll from every guest handler answered `true`: a guest
that checks it to decide whether to keep working silently stopped working. Fixed with an RAII
`ToolCancelBinding` declared *after* the instance mutex guard, so it drops *before* it and no other
call can observe the gap. **This generalises to the whole WIT seam:** every pi surface where a
parameter cannot cross the Component Model boundary is re-expressed as host-side instance state plus a
guest poll import (`signal` → `is-cancelled`, `AbortSignal` → `is-run-cancelled`, the `onUpdate`
callback → `emit-update` + `take_tool_updates`). **It is a bug generator, not a one-off.**

**J. An unbiased `tokio::select!` picks at RANDOM; a JS race cannot. (`EXT-M02`.)**

`EpochDriver::spawn` raced `token.cancelled()` against `iv.tick()` with no `biased;`, and a freshly
built `tokio::interval` fires its **first tick immediately** — so whenever the token was already
cancelled at spawn, both arms were ready on the very first iteration and shutdown depended on a coin
flip (expected ~2 iterations: a flake, not a hang). **Audit result, recorded so it is not redone:**
this was the only unbiased `select!` in cyrup-ext / cyrup-ext-sdk / cyrup-sdk; the other eleven carry
`biased;`, and **`caps/proc.rs:297`'s unbiased one is CORRECT** — its loop re-evaluates the
authoritative condition at the top of every iteration and both arms are no-ops. **Do not "fix" it.**

**K. Rust has no object spread, so every wrapper is a hand-written list that silently rots — and the rot is INVISIBLE to the obvious test.**

pi's `wrapRegisteredTool` is `return { ...tool, execute }` (`core/extensions/wrapper.ts:21-22`): every
field survives **by construction**, including fields added years later. `impl Tool for RegisteredTool`
must name each method, and its list of eleven omitted `constrained_sampling` — so a WASM guest's
declaration was read off the descriptor and discarded one frame later (`PROV-011`). **Because a
dropped delegation returns exactly the trait default, `assert_eq!(w.x(), inner.x())` compares the
default against the default and passes with the delegation deleted.** `TOOL-024` fixed nine instances
of that vacuity; `constrained_sampling` was added to the trait afterwards and made a tenth. **Fixing
the fixture once does not immunise it — the "every fixture value is DISTINCT and non-default"
invariant must be re-established in the same commit as every new trait method.**

**L. A JS `Map` is TWO data structures, and this port keeps translating it as one.**

`seenInboundMessages` (`ICOM-017`) uses `has(key)` (hash) **and** `keys().next().value` (insertion
order) inside the same sixteen-line function: the cap eviction is "forget the OLDEST", and a bare
`HashMap` silently makes it "forget one at random". The file had already recorded this trap for
`activeTools` (`v0.10.1 index.ts:677` reading `activeTools.values().next().value`) and it was hit
again. **Standing rule: any pi `Map` whose code touches `.keys()`, `.values()` or `.entries()` needs
an explicit order carrier in Rust.** Register entry **D**'s `serde_json::Map` case is the same
mechanism at the serialization layer — and worth a dedicated sweep rather than one item at a time,
since every user-authored JSON file this port round-trips through `serde_json::Map` gets its key order
rewritten (`models-store.json` was `CFG-042`; `keybindings.json` already carries a bespoke ordered
type).

**M. `Iterator::all` short-circuits; three effectful calls ANDed together do not.**

pi's `ensurePermissionForwardingLocation` evaluates three `ensureDirectoryExists` calls as
unconditional bindings and ANDs the results at the end (`:803-805` → `:808`), so a spool with three
broken directories reports **three** causes. The natural Rust transcription
(`[..].into_iter().all(..)`) reports one and stops. **The RETURN VALUE is identical, which is exactly
why this survives review — only the side effects differ.** Any port of a JS expression that ANDs
several effectful calls needs a fold. (`PERM-033`.)

**N. A boolean predicate has nowhere to put a diagnostic; the JS guard function it replaced logged before returning `false`.**

`forwarding::response_is_bound` had a real caller and correct constant-time semantics — but the caller
was `Option::filter`, which **discards the reason**. Upstream cannot lose it, because the same check is
a function that logs first. **Any Rust `bool` predicate standing in for a JS guard that also logged is
a place to look for dropped diagnostics**, and the security-relevant instance is exactly this one: a
forged or misaddressed forwarded response was discarded leaving only an all-null `response_received`
entry. (`PERM-033`.)

**O. `serde_json::from_str::<T>` collapses two upstream error classes into one.**

pi separates `JSON.parse` failure (catch → `Failed to read …`, **with** the cause) from a field-shape
rejection (→ `Ignoring invalid … format in …`, **without** a cause, because it holds a parsed object
and not an error). A single typed deserialize cannot tell them apart, so a faithful port must go
through `serde_json::Value` first. **Every ported `readX()` that has both a catch and a validation
ladder has this trap, and it is silent:** one plausible message where upstream emits two distinct ones,
with the `error` key wrongly present or wrongly absent. (`PERM-033`.)

**P. A correlation table keyed by id is deliberately REUSED across operations upstream; "tidying" it into two tables type-checks and then hangs.**

pi's `cancelMessage` resolves through the **same** `pendingSends` map as `send`, keyed by the
**cancelled** message's id, because the broker answers a cancel with `delivered { messageId }` naming
that id. A Rust port that introduces a separate `pending_cancels` table compiles and then waits
forever on every cancel — the ack arrives and matches nothing. (`ICOM-017`.)

**Q. A mechanism ported one level up or down turns an invariant into an obligation.**

pi's `showWarning` builds `Warning: ${message}` **inside** the function
(`interactive-mode.ts:3885-3889`); cyrup's `Entry::Warning` renders verbatim, so the prefix became a
**per-caller** duty. Two of three callers complied; `main.rs`'s `modelFallbackMessage` push did not, so
a credential-less first run rendered a bare sentence where pi renders a labelled warning. (`TUI-062`.)
The same shape appears in `findAutoloadDeltaBase` (`CFG-026`): pi computes each scope's identity
against **its own** base (`package-manager.ts:1307` vs `:1311`), so a relative local path never pairs —
cyrup's raw-string comparison paired them, i.e. **cyrup was more permissive than pi**, and the in-code
doc comment asserted the opposite of upstream's behaviour.

**R. `??` and `||` differ on the empty string, and pi uses both within sixty lines.**

`result.reason ?? "Message may not exist..."` (`v0.10.1 index.ts:1955`) **keeps** an empty-string
reason, where the `||` fallbacks nearby replace it. `unwrap_or_else` on `Option` is `??`; reproducing
`||` needs an extra `.filter(|s| !s.is_empty())`. (`ICOM-017`.)

**S. `std::process::Command` inherits every UNNAMED stdio handle, so a test fixture can hold the harness's pipe open.**

nextest waits `leak-timeout` for EOF on fds 1/2 after a test process exits, and EOF needs every copy of
the write end closed. Naming a handle `dup2`s over the harness's copy — **only an omitted handle can
leak**. One test (`path_probe_is_bounded`) named `.stdout(piped())` only, handed its child the
harness's stdin and stderr, and left it alive for 30 s on an error arm — producing an intermittent
`LEAK` on the very test whose purpose was proving the production probe reaps (`TOOL-042`). **Reviewing
a spawn for what it SETS can never catch this; only a rule about what it must not leave UNSET does.**
`.output()` is safe (it overrides all three); **`.status()` is NOT** — any future lint that exempts
"terminal builder methods" as a class re-opens the leak through `.status()`.

**T. Two functions ported from two different upstream files can leave a value classified by neither.**

`is_local_path` (from `paths.ts:41-55`) treats `github:user/repo` as **non-local**, while
`parse_git_url` (from `git.ts`) rejects it because the prefix is not `git:` and the scheme is not
https/http/ssh/git — so the source is simultaneously "not a local path" and stored as one.
Upstream's `parseGitUrl` reaches `hostedGitInfo.fromUrl`, which resolves the shorthands. (`CFG-052`.)

## Structural defect J — the merge gate does not cover the `cyrup-it` harness

New this pass, and it is a property of the gate rather than of any item.

`crates/cyrup-it` is `required-features = ["it"]` (its own `Cargo.toml:26-34` says so), so
`cargo test --workspace` / `cargo nextest run --workspace` **does not build or run it**. The 6440-test
figure therefore gives **zero coverage of the broker-socket seam tests**. **UPDATED 2026-08-14 (sweep 6).** The four `cyrup-it` assertions this section cited as contradicting
production (`tests/intercom/tool_actions.rs:319`, `:372`, `:502`,
`tests/intercom/intercom_command_transcript.rs:144`) **no longer do** — they were re-read at HEAD and
match production with no trailing period (`ICOM-026`, closed as REFUTED). **The structural defect is
unchanged and is now filed in its own right as `ICOM-053`**, because it is a statement about the GATE
rather than about those tests, and burying it inside a closed test-defect row is how it would be lost.
Two further consequences are now on the record: **`EXT-025` cannot be closed by deleting its dead
methods** — the only callers are tests in this un-built crate, so the breakage would land silently, and
it needs ONE agent owning cyrup-ext + cyrup-session-svc + cyrup-it in a single commit; and **the 0.6
and 0.7 `HOST_WORLD` bumps have never been instantiated against a real guest**, because the Tier-1
fixture component lives here too, with an opaque wasmtime LINK error as the failure mode. `PERM-022`
moved into the same crate and inherits the problem. **Either the gate gains a second invocation with
`--features it`, or every assertion in that crate should be treated as unverified.**

## Test architecture — recorded because it invalidates path citations everywhere

The integration tests were relocated into their crates as unit tests (`63d729a` / `c3982b5` /
`d973906`): **310 integration binaries → 6 + 8 gated**, behind the new `cyrup-it` harness crate, with
the gate now at **6699 tests, 6699 passed, 7 skipped, in 16.3 s at HEAD `bdcb0d0`** (**6440 in 16.4 s**
was this section's own previous figure; the inherited 3932 and the repro pass's 6387 are older still —
all three are superseded). **Every `crates/<crate>/tests/<x>.rs` citation in this directory
is stale unless it names `cyrup-it`** — the affected items are enumerated in each area file's
reconciliation block. Areas 02, 03 and 04 additionally have no out-of-crate `tests/` directory at all.

## What a planner should do next, in order

1. ~~**`SEAM-061`** — one agent, both crates (07 + 08). It is the only high whose fix is fully~~
   ~~understood and fully blocked on coordination.~~ **STRUCK 2026-08-14 (sweep 6): closed as REFUTED — it was already landed at HEAD in both crates. The ranked actionable set is now `SESS-040` and `PROV-047`.**
2. **The one-line residuals**, which are cheap and are what makes three landed features actually
   reachable by a user: `PROV-047`'s `configure_http_proxy` call in the session-svc builder;
   `EXT-037`/`EXT-038`'s two `LiveHostServices` impls (`commands()` off
   `AgentSession::slash_command_catalog`, `all_tools()` off the dynamic tool registry, so built-ins
   appear); `SESS-035`'s doc line. Each is small, mechanical, and named exactly in its row.
3. **Build and instantiate the Tier-1 WASM fixture.** `HOST_WORLD` moved `0.5 → 0.6` in sweep 2 and
   **nothing has proven it against a real guest** — host and guest agree at the type level and no
   further. A failure will present as an opaque wasmtime link error.
4. **Move the items that cannot be closed where they are filed**, rather than letting a fourth pass
   re-derive the same blocked plan: `EXT-025`, `EXT-003`, `EXT-059`, `EXT-013`, `EXT-041`, `EXT-053`,
   the residuals of `EXT-039`/`040`/`019`, `TOOL-024`, `PERM-011`, `PERM-022`, `SEAM-073`(b).
5. **Read `caps/{http,fs,proc}.rs` and the `is_trusted` gate** (area 06, blind spot 4). Roughly 1000
   lines, never read by any pass, and now the only part of `host/services.rs` nobody has walked —
   in the same file where `EXT-054` found the entire manifest capability model inert.
6. **Re-derive every remaining `upstream-drift` row at `v0.83.0` before scheduling it.** Four more
   kind corrections landed this pass (`DRIFT-013`, `DRIFT-029`, `DRIFT-046`, plus `DRIFT-033`'s
   scope), all in the same direction: filed as version lag, actually a port omission inside the
   baseline. **This file's default assumption about that class is wrong more often than it is right.**

---

> **REGENERATED 2026-08-12 (second edition, same day) — cyrup HEAD `04c1ba2`** (last code commit;
> repo HEAD `a9000b1` is docs-only, branch `david/cyrup`), against **pi `v0.84.1`**,
> **pi-subagents `v0.47.1`**, **pi-permission-system `v0.8.0`**, **pi-intercom `v0.10.1`**.
>
> **This edition is derived from the twelve area files AFTER the repair pass** that applied the
> 17-finding completeness critique. Severities moved, six items were reclassified as trackers,
> duplicates gained machine-readable `duplicate-of:` markers, and 31 further items landed from four
> upstream surfaces no file in this directory had ever named. **Every count in the previous edition
> is wrong.** Nothing below is carried forward; every figure was recomputed from the twelve
> `## Open items` tables at their current contents.
>
> **The four headline results.**
>
> 1. **The "zero criticals" headline is gone. There are six**, and they were not created by new
>    code — they were created by applying `README.md:106-107`'s own definition to items already in
>    the backlog. `AGENT-020`, `TUI-027`, `EXT-054` and `PERM-009` were re-rated on their own text;
>    `TUI-042` and `TUI-043` are new. Four of the six are silent data loss in the prompt editor or
>    the agent seam. **The previous edition's result #1 was an artifact of not applying the scale.**
> 2. **Highs went 14 → 22**, mostly from one axis nobody had run: area 08's sweep of
>    `pi/packages/coding-agent/src/cli/` alone produced five highs on the pre-launch startup surface
>    (`SEAM-061`…`SEAM-065`). Area 08 now carries seven open highs, more than any other area.
> 3. **The open count is 448 raw IDs / ~420 distinct defects / 9 trackers** — see *The open set*.
>    The previous edition published 426 as a floor while it was simultaneously inflated by known
>    duplication; both figures are now published separately and labelled.
> 4. **117 closed against 207 filed.** The port converges by *severity*, not by item count — and
>    this pass it did not even converge by severity, because the severity scale was being applied
>    incorrectly. That correction, not new breakage, is what produced the six criticals.
>
> **Claims struck from the previous edition** (each checked at the source this pass):
>
> - ~~"`PARITY-GAPS.md`'s own header is stale — it records `pi-subagents` latest v0.43.0 and
>   `pi-intercom` baseline v0.7.0 / latest v0.9.2"~~ — **FALSE, struck.** `PARITY-GAPS.md:3-8` and its
>   baseline table at `:20-25` record **v0.47.1** and **v0.10.1**, and the file *opens* by explaining
>   that correction and the `pi-intercom` v0.9.2 baseline. This ledger was the stale file, in both
>   places it claimed otherwise (previously at `:282-284` and in structural defect D).
> - ~~"Catalog lag is unresolvable from this workspace… there is no in-tree regeneration source"~~ —
>   **FALSE, struck.** `packages/ai/scripts/generate-models.ts` (2,733 lines) plus `model-data.ts`,
>   `models-dev-reasoning-options.ts`, `check-model-data.ts`, `scripts/diff-model-catalog.mjs` and
>   `scripts/publish-model-catalog.mjs` all exist at **both** tags, and pi's root `package.json:24-30`
>   exposes `generate:models` / `hydrate:model-data` / `generate:model-catalog` / `diff:model-catalog`
>   / `check:model-catalog`. Only the generated **output** (`packages/ai/src/providers/data/`) is
>   gitignored (`.gitignore:11`). The work is `PROV-018` (tooling) and it is tractable today;
>   `DRIFT-009` has been rewritten to defer to it.
> - ~~"Zero critical items remain open"~~ — see headline 1.
>
> **This is a STATIC analysis.** Nothing in this file or in any area file was reproduced by running
> cyrup or pi. No binary was built, launched or tested for this pass or the repair pass; no Rust or
> TypeScript source was modified. Every item is evidenced by reading both sources at a named tag.
> Severity and effort are judgements, not measurements. Every `Verify` line in every area file is a
> design, not an observation — and for any TUI item, a design that is **not satisfied by a
> `TestBackend` unit test**: the standing rule is that TUI work is not done until it has been run in
> a real terminal.

---

## The open set at `04c1ba2` — two figures, both published

> **SUPERSEDED 2026-08-14.** Both figures below were computed before the repair pass finished
> growing the tables and neither was re-derived afterwards; the tables actually contained **463**
> counted rows, not 448. The current figure is **173 open — 0 critical, 3 high, 75 medium, 95 low**.
> See *RECONCILED 2026-08-14* at the top of this file. The **deduplication reasoning** below (the
> `duplicate-of` census and the F4 cluster) is still valid and was not re-run; a deduplicated figure
> for 173 has not been computed.

The previous edition called 426 a floor while its own cluster tables documented ≥25 IDs of
double-count inside it. Both numbers are now stated, each labelled, so the count can be used.

| | critical | high | medium | low | **total** |
|---|---|---|---|---|---|
| **raw ID count** (severity-bearing rows in the twelve `## Open items` tables) | **6** | **22** | **197** | **223** | **448** |
| less area-12 rows carrying a machine-readable `duplicate-of:` an in-census ID | — | −1 | −3 | −11 | **−15** |
| less cross-area cluster excess in areas 01–11 (cluster F4, editorial) | — | — | −6 | −7 | **−13** |
| **deduplicated distinct defects** | **6** | **21** | **188** | **205** | **~420** |

**Plus 9 `tracker` rows, excluded from every figure above** (see *Trackers*), and 2 `partially-closed`
rows in area 02 (`AGENT-S01`, `AGENT-S04`) that are listed for provenance and counted nowhere.
Total rows across the twelve tables: **457**.

The deduplication is exact for the 15 area-12 rows — each carries `duplicate-of: <ID>` in its row,
its status row and its body's `Kind` line, and the owning area is named. The 13 cross-area
subtractions are **editorial**: they come from cluster F4 below, where a defect that is genuinely one
piece of work carries two or three IDs in different areas. Some F4 members are separable halves
rather than true duplicates (`SEAM-051` is a flag, `CFG-021` is a settings key, `TUI-019` is the
renderer), so ~420 is itself a soft figure. **Use 448 for "how many rows must be dispositioned" and
~420 for "how many distinct things are wrong."** Neither is a total — see structural defect C.

**Effort profile: S 286 · M 130 · L 32.** 64% of the backlog is effort `S`. Of the 28 criticals and
highs: **15 `S`, 12 `M`, 1 `L`** — the top of the backlog is unusually cheap.

**Movement since the first 2026-08-12 edition: 426 − 9 + 31 = 448.** Nine items left the count as
trackers — `PROV-004`, `AGENT-028`, `SESS-038`, `SEAM-058`, `SUBA-005`, and area 12's `DRIFT-022`,
`DRIFT-023`, `DRIFT-032`, `DRIFT-040` — and 31 items entered it. **No ID was renumbered, merged or
deleted to produce any of this**, in any of the twelve files; every tracker keeps its ID, its row and
its full body.

---

## The actionable set — 6 criticals + 22 highs

> **SUPERSEDED 2026-08-14 — every row below is dispositioned.** All six criticals and 31 of the 34
> highs are closed; the actionable set is now three items (`SEAM-061`, `SESS-040`, `PROV-047`) and is
> tabled at the top of this file. The section is retained because each row's *fix sketch* is still the
> best short statement of what the work was, and because several rows record a refutation that must
> not be lost — in particular row 1 (`AGENT-020`, refuted by measurement) and row 6 (`EXT-054`).
> **Do not plan from the ranking below.**

Ranked by the criterion stated in *Ranking proposal*. One row per item, written so a planner can
schedule without opening the area file. Every row names the file and the fix.

| # | ID | area | sev · effort | why it matters, and what to do |
|---|---|---|---|---|
| 1 | **AGENT-020** | 02 | ~~**crit**~~ **low** · S | **⚠ REFUTED AS RANKED, 2026-08-13 — this row's premise was measured and is false; the ranking below it is stale.** Typing during a live stream queued and delivered the message **5/5 times**, including four attempts timed at the settle boundary (`REPRO-LOG.md`). The code path is real and unchanged at HEAD, but the TUI's steering path does not enter the drain-before-latch window; the loss is reachable only through `AGENT-030`'s sub-millisecond race and was not observed. Severity lowered critical → low. **Whoever next re-ranks this ledger must promote a different item into position 1.** Original text follows. ~~**Silently destroys a user-typed steering message.**~~ `Agent::continue_run` (`agent.rs:1637`) drains the steering queue at `:1646` and the follow-up queue at `:1650` **before** `start_run` (`:1659`) claims the latch at `:1672-1682`; on `Err(RunActive)` at `:1681` the drained `Vec<AgentMessage>` is dropped with no error, log or retry. pi guards first — `agent.ts:351-353` **@v0.83.0** throws before the drains at `:361`/`:367` (`:362-364` is the v0.84.1 offset; the code is byte-identical, the line numbers are not). **Fix:** hoist the `is_running()` check to the top as a fast path, AND — the load-bearing half, since the fast path is racy in Rust where pi gets atomicity from single-threaded JS — capture each drained vec and restore it via a new `PendingQueue::push_front` (`queue.rs`) before propagating. Test that fails today: `steer('keep-me')`, hold the latch, `continue_run()`, assert `Err(RunActive)` **and** `has_queued_messages()`. |
| 2 | **TUI-042** | 07 | **crit** · S | **Undo silently sends the literal marker text to the model instead of the pasted content.** `Snapshot` (`editor.rs:71-78`) carries `lines`/`row`/`col` and **not** `pastes`; backspace (`:814`) and delete (`:852`) erase the registry entry after the snapshot is pushed, so `undo()` (`:748-756`) restores the visible `[paste #N +42 lines]` text while `pastes[N]` stays gone, `marker_at` (`:663-694`, ends `self.pastes.get(&id)?`) no longer matches, and Enter ships ~20 characters of marker. **Fix:** add `pastes: BTreeMap<u32,String>` + `paste_counter` to `Snapshot`, clone in `snapshot()` (`:716-719`), restore in `undo()`; carry the same on `history_draft` (`:93`, `:1199`, `:1218`), which reuses `Snapshot`. Upstream `editor.ts:216-220` + `:2012-2030`, byte-identical at both tags **at the same line numbers** (checked). |
| 3 | **TUI-043** | 07 | **crit** · S | **One Ctrl+W after a large paste drops the paste.** `word_left_target`/`word_right_target` (`editor.rs:1074-1128`) classify only via `is_word_char` (`:1637-1639`) and never call `marker_covering` (`:697-712`), so Ctrl+W at the end of `[paste #1 +42 lines]` deletes the single `]`, the marker stops matching, and Enter sends the 19-char fragment. **Fix:** port `findWordBackward`/`findWordForward`'s `isAtomic` branches (`word-navigation.ts:44-46`, `:97-99`, present at v0.83.0, whose `isAtomicSegment` declaration at `:9-14` carries pi's own paste-marker comment) and make `delete_word_backward`/`delete_word_forward` (`:874-892`) drop the registry entry the way `backspace()` does at `:814`. Ship with #2 and `TUI-044`. |
| 4 | **PERM-009** | 10 | **crit** · S | **A configured `tools.bash: deny` is defeated and the allow-listed command executes.** `extension.rs:1651-1653` adds a cyrup-only `bash` bypass to `should_expose_tool`; upstream's `shouldExposeTool` has a read/skills bypass and nothing else at **both** `index.ts:2049-2075` @v0.7.1 (the ported baseline, so this is an in-baseline parity bug, not drift) and `:1790-1816` @v0.8.0. Because `manager.rs:205-215` resolves a bash **command** rule above the tool-level state — its own comment says "command rules OUTRANK the tool-level bash fallback" — `tools.bash: deny` + `bash: {"git status": allow}` leaves bash advertised **and** runs it. **Fix:** delete `:1651-1653` and the justification comment at `:1624-1631`; refresh the citation. No test pins the divergence (`tests/context_hygiene.rs:128-152` denies `write`), so the suite goes green on the deletion. |
| 5 | **TUI-027** | 07 | **crit** · M | **A mistyped key writes persisted user data.** `/tree` has no text search, so `z`/`x`/`e`/`t` are bound to characters pi types *into* that search; `e` opens the inline label editor, which captures every key, and Enter emits `SelectorOutcome::Apply(entry_id + FIELD_SEP + label)` (`tree_selector.rs:540-546`) → `app.rs:3306-3307` → `app.rs:3763-3767` → `host_services.set_label` → `manager.append_label` — the same live path an extension's `setLabel` uses, into the session JSONL. **Fix:** add `search_query` to `TreeSelector`, accumulate printable non-control chars in the fall-through arm (replacing the digit-filter at `:867-873`), filter, clear on Cancel, pop on backspace (`tree-selector.ts:1079-1100`); rebind `TreeKeymap::default` (`keymap.rs:908-915`) from `z/x/e/t` to alt+left / alt+right / shift+l / shift+t; add the seven `app.tree.filter.*` ids to `TreeAction::from_id` (`:887-895`). Depends on `CFG-048`. |
| 6 | ~~**EXT-054**~~ **FIXED 2026-08-13** | 06 | **crit** · M | **Every WASM guest gets the full host surface; the documented per-extension sandbox is inert.** `load_discovered` (`facade.rs:1166-1184`) holds `disc.manifest` and calls `load_wasm(id, &bytes, services)` — whose signature (`facade.rs:1063-1070`) has no manifest parameter — so `capabilities.{fs,exec,net,ui}` provably cannot reach instantiation and narrows nothing. Gated only by the coarse `origin.is_pre_trust() \|\| project_trusted` check. **Fix:** take `&ExtensionManifest` (or a resolved `Capabilities`) in `load_wasm`, pass `disc.manifest`, seed `ProcCaps`/`HttpCaps`/`FsCaps` in `GuestState` construction from the grant instead of `Default`, and make the exec/net/ui host imports in `host/live.rs` return a typed denial when the bit is false. Deny-by-default: the loader's two `capabilities: Default::default()` synthesis sites (`loader.rs:213`, `:259`) must stay the EMPTY grant. Ships with `EXT-055`. **Blast radius today is zero shipping guests — which is the argument for doing it *before* the first third-party component, not after.** **FIXED 2026-08-13 with EXT-055** — `load_discovered` now calls `load_wasm_with_caps(.., &disc.manifest.capabilities)` (`facade.rs:1223`); the grant crosses as data into `GuestState::with_capabilities` and is enforced host-side at the `exec`/`proc`/`http-client`/`ui` import boundary (`host/live.rs:53-88`) plus `FsCaps` for `ext-fs`. Both `loader.rs` synthesis sites are now the explicit `Capabilities::none()`. `load_wasm` keeps its signature (manifest-less host-internal entry, `Capabilities::host_granted()`), so no caller changed. Evidence: `crates/cyrup-ext/tests/manifest_capabilities.rs`, 9 tests, 5 RED before the one-line revert. **Residual filed:** `AgentSession::load_wasm_extension` (`cyrup-session-svc/src/session.rs:3956-3962`) is still a full-authority manifest-less load reachable as public API — outside this pass's file ownership. |
| 7 | **AGENT-030** | 02 | high · M | **Two concurrent runs on one session.** `AgentSession::prompt` (`session.rs:627`) and `prepare` (`:854`) gate on `is_streaming()` → the agent's **per-run** flag, which `SettlementGuard::drop` clears at `cyrup-agent/src/agent.rs:1441` the moment each run settles — so a prompt landing in the post-run gap (auto-retry, auto-compaction, queued continuation) starts a SECOND run where pi queues it as steering. The session already owns the right latch (`driver_tx`, set at `:686`, dropped after the post-run loop at `:739`) and consults it only in `is_idle()` (`:601-603`). pi's `_isAgentRunActive` spans `_handlePostAgentRun` and every `agent.continue()` (`agent-session.ts:1062`/`:582`/`:876-877`/`:1159` @v0.83.0). **Fix:** add `is_run_active()` reading `driver_tx`, switch `:627` and `:854` to it, route post-run-gap submissions to `queue_steer`/`queue_follow_up`. **Must land in the same change as #1** or the loss just moves to the other branch. |
| 8 | **PERM-023** | 10 | high · S | **The gate never attaches, so configured denies are inert.** `is_installed` (`extension.rs:2159-2175`) probes env var, policy file and `config.json` and nothing else, while `manager_paths_for` (`:390-401`) wires `agents_dir` and `manager.rs:500-503` loads `<agents_dir>/<agent>.md` as an **enforced** policy layer. An operator whose only artifact is a persona's `permission:` frontmatter gets `is_installed == false`, no extension attached, and silently inert deny rules. **Fix:** return true when `<agent_dir>/agents/` or `<cwd>/.cyrup/agent/agents/` exists and is non-empty. Neither directory is ever written by this crate, so it carries none of the self-footprint hazard that produced `PERM-002`. Verify serialized on `ext_config::env_lock()`. |
| 9 | **TOOL-039** | 04 | high · S | **`CYRUP_SHELL` silently redirects every model-issued `bash` call to an arbitrary interpreter.** `ops/shell.rs:101-105` makes it the FIRST arm of `ShellConfig::detect()`, ahead of the `/bin/bash` probe (`:108-110`), `which_bash` (`:111-113`) and the `sh` fallback (`:114-118`); `detect()` is the default path for both `ToolRegistry::with_builtins` (`registry.rs:54`) and `Backend::default()` (`ops/mod.rs:359-361`); `session_env_scrub_keys()` (`config.rs:41-48`) is built from `SESSION_ENV_SUFFIXES` (`config.rs:31-32`) crossed with `CYRUP_`/`PI_`, so it **structurally cannot** contain `CYRUP_SHELL`, and a subagent run is a real re-exec; nothing records the resolved interpreter anywhere. The value goes straight to `get_bash_shell_config`, so the substitute need not be a shell. pi's `getShellConfig` (`utils/shell.ts:67-120`, byte-identical at both tags) reads **no** env var as a shell selector. **Decide in ONE change with `TOOL-007`:** (i) delete the arm and require the `shellPath` setting — three lines, pi's shape, recommended; or (ii) keep it and do **all four** of stamp a `[CYRUP-DELTA]`, report the resolved interpreter at session start and in bash result details, add a second explicitly-named scrub group (it does not fit the `{CYRUP,PI}_<SUFFIX>` shape), and validate the path per `shell.ts:73`. Half of (ii) is not an option. |
| 10 | **SEAM-065** | 08 | high · M | **Trust is resolved pre-launch, inverting pi's tier order, so the extension `project_trust` hook is skipped.** `main.rs:325-329` calls `resolve_startup_ui` before any runtime exists; `main.rs:1142-1162` resolves trust from store + default policy, prompts, and sets `config.trust_override` (`:1159`) — which short-circuits `builder.rs:495-499` (`if cfg.trust_override.is_none() && has_resources`), so `pre_trust_extension_verdict` never runs and the hook only gets a say when the user **cancels**. pi orders the extension tier above the store, the default policy and the prompt (`project-trust.ts:46-95` @v0.83.0, identical at v0.84.1). **Fix:** delete the trust block from `resolve_startup_ui` and give `SessionServiceBuilder` a `with_trust_prompt` callback invoked only on `TrustOutcome::NeedsPrompt`. Also retires `builder.rs`'s `saved: None` and its "no trust store is wired" warning. Latent until someone ships a trust-policy extension — and silent when it fires. |
| 11 | **SEAM-064** | 08 | high · S | **A user cannot answer a security prompt without recording a permanent verdict.** `main.rs:1155` passes `trust_options(&dirs.cwd, false)`; the flag gates both "(this session only)" rows (`trust.rs:356-363`, `:370-377`), so the startup prompt renders three options, every one with a non-empty `updates`, and `run_trust_prompt` persists them unconditionally (`startup_ui.rs:266-268`) — including a permanent lockout. pi's **pre-launch** path passes `includeSessionOnly: true` (`project-trust.ts:32`) while its in-app selector does not, so cyrup's other call site (`session.rs:3255`) is correct and must be left alone. **Fix:** one production character — `true`. Update `startup_ui.rs:504-537` to assert the five-option order and that a session-only index yields empty `updates`. |
| 12 | **SEAM-062** | 08 | high · S | **The pre-launch `--resume` picker invites a rename, accepts it, repaints the row with the typed name, and drops it.** `run_resume_picker`'s `on_apply` (`startup_ui.rs:129-138`) matches only `Delete`; the rename payload falls through. pi disables rename entirely on this surface (`session-picker.ts:48` passes `showRenameHint:false` and no `renameSession` callback, so `canRename` is false and the handler bails). **Minimum fix:** `set_show_rename_hint(false)` plus a new `SessionSelector::set_rename_enabled(bool)` gating `SessionAction::Rename`. **Preferred:** handle the outcome by opening the target and appending `session_info`, reusing `session.rs:3355-3365`. Same class as #5 — typed text accepted, echoed, discarded. Verify by relaunching after a rename **in a real terminal**. |
| 13 | **SEAM-063** | 08 | high · M | **Session delete permanently unlinks where pi routes through `trash`, and the failure is swallowed.** `rg -ni trash crates/` returns zero; both `startup_ui.rs:133-137` and `cyrup-session-svc/src/session.rs:3343-3347` bare-unlink, and the startup site discards the `io::Result` so a failed delete still reports success. **Fix:** one `delete_session_file(path) -> Result<DeleteMethod, String>` helper — spawn `trash` first with pi's `["--", path]` guard, success on exit-0 **or** the file having vanished, else `std::fs::remove_file` — called from both sites, propagating the method so `app.rs:4025` can say "moved to trash" vs "deleted". Verify with a stub `trash` on PATH for all three arms, then a live run. |
| ~~14~~ | ~~**SEAM-061**~~ **CLOSED 2026-08-14 — REFUTED (sweep 6)** | 08 | ~~high · M~~ | **The `--resume` picker lists every project's sessions under a header that says "Current Folder", with a `tab scope` hint that does nothing.** `gather_session_infos` (`main.rs:1259-1268`) concatenates the cwd listing and the cross-project listing into one vector; `run_resume_picker` hands it to a `SessionSelector` defaulting to `scope=Current`, so the cwd column is off and the advertised toggle is inert — no `SessionAction::ToggleScope` exists. **Fix:** take pi's two loaders separately, add `ToggleScope` bound to Tab, flip `show_path` with the scope (pi's `showCwd`), make the hint conditional, thread `SessionListProgress`. Both halves must land together or the screen keeps lying. Verify with two project dirs **in a real terminal**. |
| 15 | **SESS-040** | 03 | high · M | **A shipped control that bills tokens and rewrites the session file does nothing, and the UI advertises it.** The indicator band renders "(esc to cancel)" at `app.rs:6044`, but `app.rs:4615-4639` handles `CompactionStart` by setting `IndicatorKind::Compaction` and nothing else — no `defaultEditor.onEscape` equivalent is installed; `rg AbortCompaction crates/` returns only the enum variant (`command.rs:32`) and its handler (`:116-118`), **no caller**, and `AgentSession::abort_compaction` (`session.rs:1677-1681`) has no production caller either. pi rebinds Escape on every `compaction_start` (`interactive-mode.ts:3074-3085` @v0.83.0) and restores it at `:3088-3095`. **Fix:** save and replace the default-editor Escape handler on `CompactionStart`, restore in the `CompactionEnd` arm, route through `command.rs:116-118`. `SESS-041` (auto-compaction still uncancellable) and `SESS-042` (a cancelled compaction is still written) are latent **only** because this has no caller — all three ship together. Verification must include a live terminal run. |
| 16 | **TUI-031** | 07 | high · M | **A turn is assembled from a context that is being rewritten under it.** A prompt typed during compaction dispatches immediately: the `AppAction::Submit` arm (`app.rs:6606-6626`) branches on `is_streaming()` only and never consults `is_compacting()`, and `AgentSession::prepare` (`session.rs:849-900`) has no compaction guard either. pi checks compaction **first** (`interactive-mode.ts:3023-3033`). **Fix:** check `session.is_compacting()` before `is_streaming()`, push onto a new `AppState::compaction_queue`, clear the editor, push pi's `Queued message for after compaction` status, suppress the optimistic echo `dispatch_submission` does at `:1750`, drain on `CompactionComplete` (`:4650-4654`). The session-layer serialization is area 03's to own; note `TUI-016` means there is currently no surface that would show a queued message. |
| 17 | **SEAM-051** | 08 | high · S | **The DEFAULT value of a v0.84.1 flag makes the binary refuse to start.** `rg tui_mode crates/*.rs` returns nothing; `--tui-mode` is absent from `KNOWN_LONG_FLAGS` (`cli.rs:757-799`), so `partition_extension_flags` (`:701-753`) captures `--tui-mode regular` as an extension flag (the value does not start with `-`/`@`), `report_runtime_diagnostics` returns fatal, and all three modes return `Ok(1)` (`main.rs:514-517`, `:662-666`, `:770-774`). No pi command line or wrapper script can launch cyrup. **Fix:** add the flag to `KNOWN_LONG_FLAGS` + `KNOWN_VALUE_LONG_FLAGS`, add a `TuiMode {Regular, Fullscreen}` value-enum to `Cli`, add pi's two error diagnostics to `apply_arg_leniency` (`diagnostics.rs:90-152`), add the help line. Accepting `regular` as a no-op and rejecting `fullscreen` with an explicit not-supported message is a legitimate interim; the rendering half is `TUI-019`/`OQ-07-1` and **must not block this**. |
| 18 | **PROV-027** | 01 | high · S | **All 9 GitHub Copilot rows on the anthropic-messages route arrive unauthenticated.** `anthropic_messages.rs:470-536` `build_headers` has no provider branch; the scheme comes solely from `is_oauth = api_key.contains("sk-ant-oat")` (`:434-437`), which a Copilot `tid=…;exp=…` token never matches, so `:524-531` emits `x-api-key`. pi branches on `model.provider === "github-copilot"` **before** the OAuth test (`anthropic-messages.ts:867-888`, verified clean at both tags this pass) and sends `Authorization: Bearer` with only the selective betas, deliberately without the Claude-Code identity headers. **Fix:** test the provider first and take the bearer path; the betas half is already correct. **One edit with #19.** |
| 19 | **PROV-028** | 01 | high · S | **`github-copilot-headers.ts` is entirely unported** — no `X-Initiator`, `Openai-Intent` or `Copilot-Vision-Request` on any of the three routes. Without `Copilot-Vision-Request` an image turn against Copilot is **rejected** (loud, normal path); without `X-Initiator` every agent-loop request is silently misreported for quota. **Fix:** port as `api/github_copilot_headers.rs` (three pure fns over `&[Message]`) and apply at `anthropic_messages.rs:470`, `openai_completions.rs` and `openai_responses.rs:412`, guarded on the provider, ordered after `model.headers` and before `opts.headers`. Upstream **@v0.83.0**: `anthropic-messages.ts:867-871`, `openai-completions.ts:638-645`, `openai-responses.ts:223-230` — the previously recorded `:646-652` was the v0.84.1 offset, corrected this pass. Needs the exact provider guard #18 introduces, so **doing them separately is strictly more work.** |
| 20 | **PROV-048** | 01 | high · S | **A lone-surrogate `\uXXXX` escape in a provider SSE frame kills the entire assistant turn.** `serde_json` rejects it, `repair_json` re-emits it verbatim (`json_parse.rs:67-75`) so `repaired == json` and `parse_json_with_repair` returns `None`, and both SSE callers treat `None` as fatal (`anthropic_messages.rs:1439-1449`, `google_generative_ai.rs:975-985`). pi's `JSON.parse` accepts it and `sanitizeSurrogates` strips it on the way out. The same weakness breaks resuming a pi-written session JSONL. **Fix:** in `repair_json`'s `Some('u')` valid-hex arm, drop an escape decoding to an unpaired surrogate (D800-DBFF not followed by DC00-DFFF, or DC00-DFFF not preceded) so `repaired != json` and the retry succeeds. Ships with `PROV-049` + `PROV-050`. |
| 21 | **PROV-029** | 01 | high · S | **Copilot and Codex login flows are fully ported and unreachable.** Each provider has a runtime half (refresh/`to_auth` only) and a login-capable flow half, and `ProviderAuth` wires the **runtime** half (`providers/github_copilot.rs:142-146`, `openai_codex.rs:129-131`), so `/login` — which resolves via `provider.provider_auth().oauth` (`cyrup-config/src/login.rs:784`) — dead-ends on the `LoginUnsupported` default at `auth/mod.rs:124-131`. Both render with the subscription marker (`github_copilot.rs:597`, `openai_codex.rs:451`). **Fix:** one field assignment per provider — add the two arms to `providers/builtin_oauth.rs:37` and delete the prose exemption at `:14-16`. Separately: either populate the flow registry (`auth/oauth/load.rs:111`, zero production callers) or delete it. |
| 22 | **SUBA-043** | 09 | high · S | **A caller's `outputSchema` is dropped without error and the run returns prose.** `subagent_tool_parameters()` (`extension.rs:6543-6690`) emits 45 properties and `outputSchema` is not among them — it exists only on the `tasks[]`/`chain[]` item schemas — and both single-run sites pin `structured_output_schema: None` (`:1934` foreground, `:2295` async). The root schema is `additionalProperties: true` and `SubagentToolParams` has no `deny_unknown_fields`, so the call is accepted. The capture mechanism `SUBA-S01` was closed to deliver is unreachable from the surface a model calls. Upstream has it top-level at `schemas.ts:349` @v0.43.0. **Fix:** add the property with the existing `sj_json_schema_object()` helper, deserialize onto `SubagentToolParams`, thread into both constructors — the runner already carries the field. Land the advertise-vs-consume guard test with it. |
| 23 | **SUBA-014** | 09 | high · S | **A skill-carrying agent is told to use a `read` tool it does not have.** `exec/mod.rs:1463-1491` builds the child's builtin allowlist verbatim with no head-injection; `rg require_read_tool` = 0. Meanwhile cyrup's own proactive-skill block (`discovery/skills.rs:273`) instructs the child to "use the read tool to load a skill's file", so an agent with an explicit `tools:` list omitting `read` plus any resolved skill silently cannot load it — and the failure surfaces as a model apology, not a config error. pi injects under an exact three-way condition (`pi-args.ts:355-372` @v0.43.0, seven live setters, all deriving it from `Boolean(resolvedSkills.length)`). **Fix:** compute `require_read_tool` from `!resolved_skills.is_empty()` (already in scope at the skill-resolution site) and inject `read` at the head. |
| 24 | **PROV-047** | 01 | high · M | **The `httpProxy` setting reaches only the streaming wire APIs.** `builder.rs:229-239` turns it into a `ProviderEnv` overlay read solely by `sse.rs:181-192` `build_client_for_target`; every other egress path uses `build_client()` (`sse.rs:140-144`), which has no proxy handling — five OAuth flows (`auth/oauth/{anthropic:443, openai_codex:552, xai:525, openrouter:372, radius:468}`), `cyrup-agent/src/proxy.rs:455` and `cyrup-provider/src/wire.rs:472` — while `cyrup-ext/src/caps/http.rs:599` is a bare `reqwest` builder with reqwest's own competing env detection. pi proxies **every** fetch via a process-global undici dispatcher (`http-dispatcher.ts:43-48`/`:79-103` @v0.83.0). **Fix:** add `configure_http_proxy()` beside `configure_http_idle_timeout`, set it at `builder.rs:1200`, turn `build_client()` into `build_client_for(target_url)` running the already-ported resolver, thread the URL through the seven call sites, add the resolver + `.no_proxy()` to `caps/http.rs`. |
| 25 | **CFG-035** | 05 | high · M | **The trust gate asks the user to trust a file cyrup will never read.** `has_trust_requiring_resources` (`trust.rs:194`, `:203-204`) prompts *because* `.cyrup/SYSTEM.md` exists, and nothing ever discovers it — the CLI flags are the only producers of the two override fields. **Fix:** add a discovery step in `builder.rs` (~`:1045-1060`) mirroring `resource-loader.ts:1022-1048` — `custom_prompt` = CLI `--system-prompt` else `<cwd>/.cyrup/SYSTEM.md` when project-trusted else `<agent_dir>/SYSTEM.md`; `append_system_prompt` = CLI (which **replaces**, per pi's `??`) else the single discovered `APPEND_SYSTEM.md` under the same trust rule; route the discovered path through `resolve_prompt_input` (`cli.rs:456`). Also correct `prompt/overrides.rs:15-16`, which documents accumulation where pi picks exactly one. |
| 26 | **SEAM-047** | 08 | high · M | **`cyrup --mode rpc` cannot be stopped by a supervisor.** `signals.rs:88-101`'s first-signal body is `session.abort(); cancel.cancel();`; the 130/143/129 codes it computes are used only on the **second** delivery at `:98-99`, and the token it fires is `main.rs:367`'s TUI input token, observed by the interactive arm only. `rpc_driver`'s `select!` (`rpc.rs:717-842`) has no cancellation arm, so `runtime.dispose()` (`run.rs:113`) never runs and no `session_shutdown` reaches extensions. pi does all of it on the first delivery (`rpc-mode.ts:366-383` → `shutdown` `:724-741`; `print-mode.ts:50-66` the same shape). **Fix:** publish the `ShutdownSignal` on a watch/oneshot, add a `cancel.cancelled()` arm to `rpc_driver` that sets `reader_open=false` so the drain-and-break at `:851` runs, add a between-message check to print/json, return 143/129 from `run.rs:22`/`:50`/`:101` **after** `runtime.dispose()`. **Ships with `SEAM-059`** (which rewrites the same function: pass `Arc<AgentSessionRuntime>` so the handler aborts the CURRENT session, not the one live at startup), `SEAM-008`, and `DRIFT-049`. |
| 27 | **DRIFT-049** | 12 | high · M | **Same defect as #26, filed in area 12** — `duplicate-of: SEAM-047`, raised medium → high this pass because a defect cannot carry two severities and a high that reads as medium falls off a planner's list. **Schedule once, in area 08.** The body is retained because it carries analysis area 08's item does not: `run_rpc` is parked on a stdin read no signal disturbs, and `cyrup-modes/src/run.rs:101-116` disposes only *after* `run_rpc` returns, so interactive/print/json survive only incidentally. It also records that `SEAM-S02` is stale — `signals.rs:97-100` does implement the repeat force-exit with pi's exact codes. |
| 28 | **PROV-030** | 01 | high · L | **A registered provider that cannot serve a single request.** `google-vertex` ships with 10 catalog models and full auth including the ADC arm, appears in `/model`, and has no wire API — every request terminates at `wire.rs:158-166` with `no API implementation for google-vertex`. This is the exact failure mode `PROV-005`'s own Fix text warned about, shipped by the sweep that closed `PROV-005`. **Fix:** port `pi/packages/ai/src/api/google-vertex.ts` as `api/google_vertex.rs` (factor the google-shared converters out of `google_generative_ai.rs` first), add `known_api::GOOGLE_VERTEX`, register the factory at `api/mod.rs:130-163`. S-sized mitigation if the port cannot land: refuse at construction time to push a provider whose catalog names an api the registry does not `contains()` (`api/mod.rs:116-119` already has the predicate). **MANDATORY in the same change:** rewrite the port-status table at `providers/all.rs:12-47`, which still calls amazon-bedrock / google-vertex / openai-codex "**pending** (NOT registered)" at `:12`/`:23`/`:34` and names all **four** at `:46-47` including github-copilot — contradicting its own table row at `:21` and flatly denying this item's premise. It is the first thing an engineer opening the file reads. Ranked last among the highs because it fails loudly and immediately. |

**Areas with zero open criticals or highs: 06 has one critical and no high; 11 and 04's tail are the
only genuinely quiet surfaces.** Area **11 (44 items, 0 high)** is quiet because its baseline was
only just corrected and its surface-driven axis has **never** been run — treat the zero as
un-swept, not clean (area 11 blind spot 10).

---

## Closure rates — the mechanism that produced 3.9% is still gone

> **SUPERSEDED 2026-08-14.** Two sweeps closed **290 rows** in one commit cycle. Whatever this
> section concluded about closure rate is answered: the mechanism that was missing was a
> whole-backlog sweep with per-item pi re-derivation, and it works. What it did *not* fix is
> verification — nothing below the gate was executed, and the analysis's own 12% error rate is now
> the binding constraint. See *The analysis's own error rate is now measured* at the top.

> The 2026-08-07 pass measured highs closing at 82% and mediums at **3.9%**, and correctly attributed
> the gap to *every commit being explicitly high-targeted*. That diagnosis was right and it is what
> predicted the change: **the rate moved when the commits stopped being item-targeted.**

**This pass: 117 items closed**, plus 4 retired by other means (`CFG-012` superseded, `SEAM-019`
superseded, `DRIFT-037` withdrawn, `DRIFT-038` → `CFG-018`). Entering open set ≈ 371.

| band | entering | outcome | rate |
|---|---|---|---|
| critical | 2 (`SEAM-021`, `SEAM-022`) | both closed | **100%** |
| high | ~17 | 14 retired, 3 carried (`PROV-027`/`028`/`029`) | **~82%** |
| medium | 178 (the 2026-08-07 audit's own figure) | ~52 closed | **~29%** |
| low | ~181 | ~50 closed | **~28%** |

The cause is structural and visible in the git log: the 64 commits in `1806375..04c1ba2` are
**area-targeted subsystem batches**, not item-targeted fixes — `8902b4f` (watchdog/missions/fleet,
39 files, ~28k lines), the ten `fix(tui): batch N` commits (`0aaca00`…`922d90c`, 103 files,
+38,826/−2,362, fifteen source files that did not exist at the old baseline), four sequential
pi-subagents batches, `0a5742d` (permission gate → v0.8.0) and `911dd59` (intercom → v0.9.2).
**Zero areas came back 100% open**; the previous pass had seven.

**The counter-fact a planner must hold alongside it, and it got worse this edition: 117 closed
against 207 filed.** 371 − 117 − 4 + 207 = 457 rows (448 counted + 9 trackers). The 31 items the
repair pass added came from four upstream surfaces — `packages/ai/utils/`, `packages/tui/src`'s
non-drawing files, `packages/coding-agent/src/cli/`, and `migrations.ts` + `core/keybindings.ts` —
that **no file in this directory had ever named**, and they yielded 2 criticals and 8 highs. That is
structural defect C behaving exactly as the README predicts. It is not evidence the work is going
backwards; it is evidence the count was never a measure of remaining work.

**The severity trend, which is the measure, is now honest and is worse than last edition claimed:**
2 criticals → 0 (real closures) → **6** (correct application of the scale). Four of the six existed
in the backlog before this pass, mis-rated.

---

## The medium/low picture — 197 medium + 223 low

> **SUPERSEDED 2026-08-14 — the current split is 75 medium + 95 low.** The clustering below is still
> the right way to read that population and was not re-derived; treat the cluster membership as a map
> and the counts as dead.

### Item kinds, re-derived from the twelve open tables

| kind | n | change vs the first 2026-08-12 edition | note |
|---|---|---|---|
| `parity-bug` | 176 | 153 → 176 | ported, then drifted — still the largest bucket by a wide margin |
| `not-ported` | 146 | 135 → 146 | predates the baseline, never built. **+11 net, but the real move is +4 reclassified INTO it from `upstream-drift`** |
| `upstream-drift` | 66 | 75 → 66 | **−9**, almost entirely area 12's kind corrections — see *Version lag* |
| `test-defect` | 23 | 23 → 23 | unchanged; still a coherent one-sweep target |
| `cyrup-original` | 21 | 21 → 21 | no upstream basis — includes the whole "advertised but inert" class |
| `stale-port` | 14 | 13 → 14 | carries behaviour upstream changed or deleted |
| `tooling` / `port-divergence` | 2 | 3 → 2 | `PROV-004` left the count as a tracker |
| `tracking` | **0** | 3 → 0 | **every `tracking`-kind item is now a tracker and outside the count** — which is what the kind always meant |

### The clusters — 420 medium+low items collapse to a small number of moves

**F1 — "advertised but inert": at least 31 ids across ten areas, one mechanism, mechanically
detectable.** Code that exists, compiles, has tests, and has **zero production callers** — or a
control the UI advertises and no code implements. This is the dominant recurring shape of the whole
analysis and it grew again this pass.

> `EXT-054` (**critical** — manifest capabilities never read) · `EXT-055` (`FsCaps::with_fs_root`,
> zero callers) · `EXT-025` · `EXT-013` · `SESS-040` (high — `AbortCompaction`, zero callers) ·
> `SESS-041` · `SESS-035` (docs-pointer section never emitted) · `SESS-033` · `PROV-029` (high —
> flow registry, zero production callers) · `PROV-032` · `PROV-040` · `SUBA-043` (high) · `SUBA-047`
> · `SUBA-054` · `SUBA-046` · `SUBA-059` · `SUBA-049` · `TUI-014` / `TUI-033` (widgets, header,
> footer stored where nothing reads them) · `TUI-044` (`Snapshot::col` written and never read) ·
> `TUI-051` (`/reload` claims to re-read `keybindings.json` in both its help text and its own source
> comment; `load_keybindings_json` has exactly one non-test caller, `main.rs:1626`, at boot) ·
> `CFG-045` · `CFG-015` · `CFG-044` · `CFG-006` · `AGENT-031` · `PERM-014` · `PERM-027` · `ICOM-041`
> · `SEAM-061` (a `tab scope` hint with no `SessionAction`) · `SEAM-062` (a rename accepted and
> discarded).

  A **schema/dispatch drift test** plus a **"no production caller" lint** would have caught most of
  these before they were written. That was suggested-order item 0 in the previous two editions, it
  has still not been built, and the class has grown every pass. **It remains the highest
  leverage-per-hour move in the backlog.**

**F2 — one WIT/ABI bump unlocks 27 ids.** The world is at `cyrup:ext@0.4.0` with a byte-identity test
tying both copies and a compile-time `ABI_FINGERPRINT`. The next bump is `0.5.0`, and `EXT-028`'s
contract says *any* export change bumps the minor — so doing these separately means twenty-seven
minor bumps and twenty-seven guest-refusal cliffs.

> `EXT-009` (+ its provider twin `PROV-042`) · `EXT-014` · `EXT-015` · `EXT-016` · `EXT-021` ·
> `EXT-023` · `EXT-024` · `EXT-035` · `EXT-037` · `EXT-040` · `EXT-042` · `EXT-043` · `EXT-044` ·
> `EXT-045` · `EXT-046` · `EXT-047` · `EXT-048` · `EXT-049` · `EXT-S04` · `SEAM-011` · `SEAM-012` ·
> `SEAM-025` · `TOOL-015` · `TOOL-016` · `TOOL-021` · `TOOL-022`.
>
> **The host-side prerequisite is unchanged and still unbuilt:** `cyrup-core/src/tool.rs::prompt_guidelines(&self) -> &[&str]`
> must return owned strings before `impl Tool for WasmTool` can ever carry guidelines. That is
> `TOOL-021` (medium, `S`). Land it in the same move.

**F3 — the test-defect sweep: 23 ids, one pass.** The suite is at zero failures, so any new failure
is signal. **Deduplicate `AGENT-019` and `DRIFT-039` first — they are literally the same test**
(`crates/cyrup-agent/tests/agent_loop.rs:327`); `DRIFT-039`'s body carries the better fix sketch (the
`agent-loop.test.ts:589-612` rendezvous), so fold that into `AGENT-019` rather than working both.
Members: `PROV-038`, `AGENT-019`, `SESS-032`, `TOOL-020`, `TOOL-024`, `TOOL-025`, `TOOL-026`,
`TOOL-030`, `EXT-032`, `TUI-N08`, `TUI-N09`, `SEAM-028`, `SEAM-030`, `SUBA-032`, `SUBA-033`,
`PERM-020`, `PERM-021`, `PERM-022`, `ICOM-025`, `ICOM-026`, `DRIFT-035`, `DRIFT-036`, `DRIFT-039`.

**F4 — 50 counted ids cover 21 defects; 28 of them are excess.** A planner scheduling by area will
book the same work twice. Area-12 pairings are now machine-readable (`duplicate-of:` in the row, the
status row and the body's `Kind` line) and account for 15 of the 28; the areas 01–11 pairings are
editorial and account for the other 13. Both are subtracted in *The open set*. Where a row lists two
owners, the ids are **separable halves**, not true duplicates — schedule both, but as one move.

| one defect | ids | owner |
|---|---|---|
| constrained sampling | `PROV-011` · `TOOL-016` · `EXT-024` · `DRIFT-018` | `PROV-011` |
| TUI / alt-screen mode | `SEAM-051` · `TUI-019` · `CFG-021` · `DRIFT-022` *(tracker)* | `SEAM-051` (flag), `TUI-019` (renderer), `CFG-021` (settings key) |
| signal teardown | `SEAM-047` · `SEAM-008` · `SEAM-059` · `DRIFT-049` | `SEAM-047` |
| cache-miss notices | `PROV-035` · `CFG-014` · `TUI-021` | `PROV-035` |
| markdown transformers | `EXT-019` · `TUI-034` · `DRIFT-015` | `EXT-019` |
| `sessionAffinityFormat` | `PROV-024` · `PROV-033` · `DRIFT-020` | `PROV-024` |
| `Current date:` footer | `SESS-019` · `DRIFT-016` · `DRIFT-035` | `SESS-019` |
| `CompactionResult.usage` | `SESS-030` · `SEAM-034` | `SESS-030` |
| `websocketConnectTimeoutMs` | `CFG-006` · `AGENT-031` | `CFG-006` |
| `labelTimestamp` on tree nodes | `SESS-S05` · `SEAM-060` | `SESS-S05` |
| parallel-tool test | `AGENT-019` · `DRIFT-039` | `AGENT-019` — *the same test* |
| Windows shell paths | `TOOL-036` · `DRIFT-046` | `TOOL-036` — **found this pass, missing from the previous F4 table** |
| embedded catalog floor | `PROV-018` · `DRIFT-009` · `PROV-039` · `PROV-004` *(tracker)* | `PROV-018` (tooling half) + `PROV-039` (provenance half) — **pairing found this pass** |
| `ANTHROPIC_AUTH_TOKEN` · radius/qwen · `deferredToolsMode:"kimi"` · usage cost · AGENTS.md double-load · `${@:-default}` · `ModelRuntime` · llama.cpp | `PROV-021`·`DRIFT-030` / `PROV-014`·`DRIFT-019` / `PROV-025`·`DRIFT-027` / `PROV-036`·`DRIFT-031` / `SESS-013`·`DRIFT-024` / `CFG-017`·`DRIFT-025` / `CFG-020`·`DRIFT-023` *(tracker)* / `EXT-027`·`DRIFT-032` *(tracker)* | the non-`DRIFT` id in each pair |

  **Area 12 is partly a duplicate index, and the census is now exact: 20 of its 34 rows duplicate an
  item another area owns; 14 are uniquely its own** (8 medium — `DRIFT-041` HTML export, `DRIFT-048`,
  `DRIFT-004`, `DRIFT-013`, `DRIFT-014`, `DRIFT-028`, `DRIFT-029`, `DRIFT-033`; 6 low — `DRIFT-010`,
  `DRIFT-036`, `DRIFT-042`, `DRIFT-045` clipboard text, `DRIFT-050`, `DRIFT-051` process title).
  **A finding worth the planner's attention:** area 12 rated its duplicates *lower* than the owning
  area in six cases, so "area 12 has no highs" was partly a bookkeeping effect rather than a property
  of the surface. `DRIFT-049` was corrected medium → high for exactly that reason.

**F5 — the closed clusters, for the record.** `C2` (no HTTP timeout or retry budget) is closed;
residual `PROV-043` (bedrock alone has no retry). `C10` (the subagent second-hop config boundary) is
closed. Do not re-schedule either.

---

## Trackers — 10 ids that propose no work, excluded from every count

These keep their IDs, their rows and their full bodies, per the stable-id rule. **A planner should
not pick one up.** They are here so the backlog stops mixing work with bookkeeping — and so that
each one's escalation condition is written down instead of implied.

| ID | area | what it is | what escalates it back into the count |
|---|---|---|---|
| `PROV-004` | 01 | The five newest catalogs were never field-diffed. Its entire Fix is "this is `PROV-018`'s `xtask gen-catalogs` and nothing else — do not re-derive by hand". | Scheduling `PROV-018`. It schedules nothing `PROV-018` does not. |
| `AGENT-028` | 02 | pi's v0.84.1 agent-harness. Its own body says "filed as scope-defining, not as loop debt"; its Fix opens "Do not port speculatively. First decide whether cyrup models pi's harness at all." | A decision that cyrup models the harness. **Answer it together with `SESS-038` — both turn on the same question.** |
| `SESS-038` | 03 | `packages/session-backends/sqlite-node`, new at v0.84.1. Nothing in `coding-agent/src` or `agent/src` imports it. | Upstream wiring it into a shipped path, or the harness decision above. |
| `SEAM-058` | 08 | pi's experimental `server`/`client` tree + `packages/protocol`/`packages/client`. Fix: "track, do not build, until upstream wires it into `main()`". Re-checked: at v0.84.1 `git grep experimentalCli` matches only the file itself and its test. | **The moment pi's `main()` references `experimentalCli`.** Its Verify line is a re-diff at the next tag, not an implementation. |
| `SUBA-005` | 09 | 27 advertised management actions against upstream's 50 (v0.43.0) / 53 (v0.47.1). Its own Fix: "this item is the ledger, not the work." | It owes two things: owners for `worktree.discard`, `approve-checkpoint`, `reject-checkpoint`, `project.open`/`status`/`close`, `mission.resolve-decision`; and a completeness assertion pinning the enum against a checked-in copy of upstream's array. Filing those makes it redundant. |
| `DRIFT-022` | 12 | TUI mode / alternate screen. Fix: "Do **not** implement yet"; Verify: "n/a while tracking". `duplicate-of: SEAM-051`. | `OQ-07-1` being answered. The behavioural cost is already carried as work by `SEAM-051`, `CFG-021` and `TUI-019`. |
| `DRIFT-023` | 12 | `ModelRegistry` → `ModelRuntime`. **Also a LEAD — neither side was re-read, in this pass or the repair pass.** `duplicate-of: CFG-020`. | Someone spending the two-sided read. The area file records the exact commands. |
| `DRIFT-032` | 12 | llama.cpp router / HF model search. Fix: "Defer until DRIFT-019 and DRIFT-009 are settled". Kind corrected `upstream-drift` → `not-ported` this pass (all files exist at **v0.83.0**), confidence medium → high. `duplicate-of: EXT-027`. | `EXT-027` being scoped — upstream ships it as a bundled **extension**, which is why area 06 owns it. |
| `PERM-017` | 10 | **Re-classified 2026-08-14 (sweep 6)** — forwarding-root agent-dir env overrides. Its own Fix is "No action while the middle levels remain meaningless" and its Verify is "n/a until triggered", which is the Trackers contract; it was counted as open work and overstated area 10's remaining set by 20%. Re-derived at the tag: `permission-forwarding.ts:62-92` @v0.8.0 resolves five levels, the three middle ones all guarded by `options.isSubagent`, and the v0.7.1→v0.8.0 diff of that file is a pure `normalizeAgentName` extraction — no level added or removed. | cyrup grows a delegated or multi-auth runtime dir, i.e. the middle levels acquire a meaning. |
| `DRIFT-040` | 12 | pi's agent-harness v2 rearchitecture. Fix: "Do **not** port now". **Also a LEAD** — its three load-bearing claims (the `agent-harness.ts` rewrite, `docs/harness-v2.md`, the sqlite-node rebuild) are still carried forward unverified. `duplicate-of: PARITY-GAPS VL-P22`. | The same harness decision as `AGENT-028`. |

---

## Version lag — four upstreams

| upstream | cyrup ported baseline | latest tag | drift window | owner | result this pass |
|---|---|---|---|---|---|
| `pi` | **v0.83.0** | **v0.84.1** | 627 files, +52,291/−17,556 | 01–08 + 12 | swept per area; every area reports its own scoped diffstat |
| `pi-subagents` | v0.43.0 *(inferred; the crate records no version string)* | **v0.47.1** | 151 files, +10,254/−1,333 | 09 | **NEWLY ANALYSED** — 11 of area 09's 24 new items |
| `pi-permission-system` | v0.7.1 | **v0.8.0** | 28 files, +4,023/−1,851 | 10 | **fully absorbed — ZERO drift items.** Its two open items are in-baseline parity bugs, one of them the workspace's only permission bypass |
| `pi-intercom` | **v0.9.2** *(corrected — every prior doc said v0.7.0)* | **v0.10.1** | true window `v0.9.2..v0.10.1` = 24 files, +2,495/−700, 14 commits | 11 | **NEWLY ANALYSED** — 13 of area 11's 24 new items |

### The two ranges no prior pass had seen

**`pi-subagents` v0.43.0..v0.47.1.** The previous docs recorded "latest v0.43.0" — i.e. no gap at
all. The src-only sweep covered **96 non-merge commits, 67 files, +4,696/−769 and 12 net-new source
files, all 12 read**, 14 commits diffed line by line. It produced `SUBA-044`, `SUBA-050`…`SUBA-060`,
`SUBA-065`, `SUBA-066`. Highlights: upstream made the bundled `reviewer` lane **read-only** (cyrup
still grants it `bash`/`edit`/`write` — `SUBA-044`); upstream bounds every async **child** at 30
minutes (cyrup has no default at all — `SUBA-051`); `subagents.modelScope.strict` now hard-rejects
an out-of-scope inherited model rather than warning (`SUBA-050`). *Not filed by rule:*
`run-fanout-budget.ts` landed on `main` after v0.47.1 with no named tag to cite.

**`pi-intercom` v0.9.2..v0.10.1.** Two corrections at once — the prior docs recorded latest v0.9.2
(one version stale) *and* baseline v0.7.0 (two versions **too pessimistic**). A citation census over
`crates/cyrup-intercom/src` returns v0.9.2 ×272, v0.7.0 ×14, v0.8.0 ×3, v0.6.0 ×1 (the `lib.rs`
banner), v0.10.x ×0, and the load-bearing v0.8.0/v0.9.x code is present **and tested**. All 14
commits accounted for; it produced `ICOM-035`…`ICOM-047`. `ICOM-012` carries the banner fix.

### Two systematic corrections that change how lag should be measured

1. **Measuring against a floating upstream HEAD over-reports lag and under-reports port omissions.**
   Re-measuring against the *named ported tag* has now reclassified **twelve** items out of
   `upstream-drift`, and **zero** in the other direction. The first pass moved `PROV-021`,
   `PROV-023`, `PROV-024`, `PROV-025`, `SUBA-017`, `SUBA-021`, `SUBA-022`, `DRIFT-014`. The repair
   pass re-derived nine of area 12's commit-hash-only items and found **six more misclassified** —
   `DRIFT-016` (`git grep 'Current date' v0.83.0 -- packages/coding-agent/src` returns nothing → the
   removal predates the ported baseline, so cyrup carries deleted behaviour: `stale-port`);
   `DRIFT-018` (`constrained-sampling.ts`, 148 lines, 7 exports, exists at v0.83.0 and is already
   imported there by five wire APIs → `not-ported`); `DRIFT-019` (three of four provider files exist
   at v0.83.0 → `not-ported`); `DRIFT-030` (`env-api-keys.ts:29`/`:76`/`:147` + `providers/anthropic.ts:5`/`:21`
   all at v0.83.0 → `not-ported`); `DRIFT-031` (`usage-totals.ts:37` at v0.83.0 with a live consumer
   at `interactive-mode.ts:5665` → `not-ported`); `DRIFT-032` (the whole `extensions/llama/` tree at
   v0.83.0 → `not-ported`, and it belongs to `cyrup-ext`, not `cyrup-provider`, because upstream
   ships it as a bundled extension). **None of these will be swept up by a rebase.**
2. **Some drift runs backwards.** `PROV-033`: cyrup carries `sendSessionIdHeader`, which pi
   **deleted** in #6496 with a documented migration to `sessionAffinityFormat` — now a three-valued
   union (`types.ts:112`) a bool cannot express. `CFG-012`: pi **adopted cyrup's** recursive settings
   merge at v0.84.1, so "fixing" cyrup toward the retired v0.83.0 spread would be a regression — the
   item is superseded, not open. `CFG-034` / the TUI scrollbar tokens: cyrup anticipated a v0.84.1
   addition.

### Known limits of the version sweep

- **117 commits past the diffed tag are unanalysed everywhere.** pi HEAD is
  `v0.84.1-117-g581d75a89` (`581d75a89`, 2026-08-13) — the count has not moved since the first
  edition, but the window is now a day older. `pi-subagents` HEAD is `v0.47.1-14-g9e9fd13`: **14
  commits past its diffed tag**, previously recorded only as the one `run-fanout-budget.ts` file.
  Area 05 names one concrete item known to sit inside pi's window and deliberately not filed:
  `getExperimentalToolSampling()`'s constrained-sampling request on the four built-in tools.
- **`packages/agent/src/harness/**` is owned by NO area file.** ~11.4k insertions / ~10.9k deletions
  in this window — the `agent-harness.ts` rewrite, a new 667-line `reducer.ts`, a new `session/`
  subtree (jsonl codec / repo / storage / state) with a 993-line conformance suite, and a new typed
  telemetry layer. `AGENT-028`, `SESS-038`, `DRIFT-040` and `PARITY-GAPS` VL-P22 all point at it, and
  **all three of the first are now trackers precisely because none of them proposes work.** This
  needs a scope decision, not another item.
- **The embedded catalog gap is tractable and mis-recorded as unresolvable.** cyrup ships 35
  embedded catalogs against upstream's 39. pi gitignores only the generated **output**
  (`.gitignore:11` → `packages/ai/src/providers/data/`); the generator and its five npm scripts are
  committed at **both** tags. `PROV-018` (medium, tooling, `M`) is the real work — an `xtask
  gen-catalogs` plus the drift check whose absence is the reason nobody noticed. `DRIFT-009` now
  defers to it, with the argument recorded for why seeding from the published pi.dev artifact is
  strictly lossier: the artifact is the *published* catalog, so it cannot reproduce what
  `generate-models.ts` computes from `models-dev-reasoning-options.ts` or from the per-provider
  compat overrides, and it yields no reproducible build step.
- **Two sibling docs must be corrected in the same edit as this one, and this time the claim is
  checked:** `PARITY-GAPS.md` §0 publishes the now-superseded census ("**426 open items: 0 critical,
  14 high, 189 medium, 223 low**") and `README.md:27-30`/`:37` declare *this* file "stale — one pass
  behind" and quote counts ("~7 actionable highs", "169 open mediums") that appear in no current
  edition. `PARITY-GAPS.md`'s **baselines** at `:20-25` are correct and were never stale — the
  previous edition of this ledger said twice that they were, and it was wrong.

---

## Deliberately out of scope — and which deferrals are now stale

### Still genuinely deferred, by decision

- **`CFG-005` — the OAuth *acquisition* cluster. Maintainer-deprioritised.** Scope has narrowed:
  `login.rs` (1,721 lines) now ports login / logout / env-key login / status / selectors, and refresh
  lives at `cyrup-provider/src/auth/resolve.rs:146-239`. The residual is **two multi-prompt api-key
  login flows** (cloudflare, google-vertex). Filed at medium / `L`, not scheduled.
- **ADR-0001 mechanism divergences** — ratatui + crossterm where pi hand-rolls a renderer; WASM
  Component Model guests where pi runs TypeScript through `jiti`. **Mechanism only.** Per the hard
  rules, where a mechanism difference *costs behaviour* it stays on the list as work.

### Decisions required — NOT deferrals, and not encoded as severities

- **`OQ-07-1` — does cyrup build an alt-screen / fullscreen TUI mode at all?** Recorded in
  `07-cyrup-tui.md`'s new `## Open questions` section. **`TUI-019` was re-rated low → medium** this
  pass: its `low` rested on "a deliberate ADR-0001 divergence", and `PARITY-GAPS.md:709` records
  ADR-0001 as **unreadable in this workspace** — which `README:208-212` forbids resting an item on,
  and which `README:213-215` would not license anyway. The `low` was encoding a decision nobody made.
  **`SEAM-051` and `CFG-021` must be fixed under either answer and must not wait on it** — a flag
  that rejects its own default value is a defect regardless.
- **`OQ-07-2` — does `cyrup/TUI-FIDELITY.md` get merged into this file with real IDs?** 464 lines,
  ~150 presentation findings against v0.84.1, no stable IDs, no status table, therefore invisible to
  this ledger. It has already cost behaviour once: its C14 recommendation to delete the `{n} queued`
  footer segment was applied, which is exactly what turned `TUI-016` from "wrong surface" into "no
  surface at all". Either answer is defensible; silence is not.
- **The agent-harness question** — `AGENT-028` + `SESS-038` + `DRIFT-040`, now all trackers. Not a
  deferral by decision; a scope decision nobody has made. They exist to force it.

### STALE — struck, and still struck

- ~~**`steer`**~~ — **STALE.** `SUBA-013` is partially closed: `CYRUP_SUBAGENT_STEER_INBOX`
  (`exec/mod.rs:1857-1868`), the child-side `SteeringInbox` (`prompt_runtime.rs:157-290`), the
  `steer` verb at `extension.rs:7825-7837`. The residual is filed work, not a deferral: **`SUBA-049`**
  (ack, delivery `mode`, `steeringRecovery` — a steer is currently fire-and-forget).
- ~~**`watchdog/`**~~ — **STALE.** `8902b4f` ported it (18–22 modules, ~18k lines, a real stdio LSP
  client, nine subscriptions, four `watchdog.*` verbs). `SUBA-011` closed. **Existence is not
  correctness** — `PARITY-GAPS` UW-3 / UW-4 / UW-5 record three no-op holes *inside* it (child NDJSON
  status never read; review never runs a model turn; the permission arbiter never runs a model turn,
  so every `ask` denies). Those are work.
- ~~**FleetView**~~ — **STALE.** `8902b4f` ported `tui/fleet*` (`fleet.rs` 2,863 lines plus five
  siblings, `/subagents-fleet` registered). `SUBA-012` closed. Residual: `PARITY-GAPS` **UW-7** — the
  fleet-status widget receives no keystrokes.
- **The same commit also ported `missions/`** (~7k lines, six modules) which no deferral list ever
  mentioned. **The deferral list was not tracking what was actually deferred.**

### Survives, but was recorded wrong

- **The "four `schedule*` verbs"** — still deferred, but it is **nine** verbs, not four:
  `schedule.create` / `list` / `show` / `history` / `pause` / `resume` / `run` / `run-due` / `delete`
  (`shared/types.ts:1968` @v0.47.1). `SUBA-016`, medium / `L`. It feeds tracker `SUBA-005`.

---

## Ranking proposal

**Criterion, stated so it can be disagreed with:** rank by *user-visible consequence when the code is
wrong*, in this order —

1. **silently destroys or corrupts something the user produced** (typed input, a paste, a label, a
   session file);
2. **silently returns a wrong result** the user cannot distinguish from a right one;
3. **a security or sandbox default that is inert** — fails open with no signal;
4. **an advertised control that does nothing** on a path that bills tokens or mutates state;
5. **a whole capability dead on a normal path** (loud, so the user knows, but blocked);
6. **cost, hygiene, and diagnostics.**

Effort breaks ties *within* a band only — never across one. A cheap loud bug does not outrank an
expensive silent one.

**0. Build the two guards before fixing anything else. Effort `S`. Highest leverage-per-hour.**
   (a) a **schema/dispatch drift test** — nothing today fails when a property is added to a tool's
   parameter schema without being wired into its dispatcher; (b) a **"no production caller" check**.
   Together they are the mechanical countermeasure to cluster **F1**, now ≥31 ids across ten areas
   and including two of the six criticals (`EXT-054`, and `TUI-027`'s label-persist path) plus four
   highs (`SESS-040`, `SUBA-043`, `PROV-029`, `SEAM-061`/`SEAM-062`). This was suggested-order 0 in
   both previous editions, it was not built, and the class has grown every pass. That is the argument
   for building it now.

**1. The silent-destruction criticals, as two shipments.**
   (a) **`AGENT-020` (S) → `AGENT-030` (M)** — one latch, two branches; fixing either alone moves the
   loss to the other. (b) **`TUI-042` (S) + `TUI-043` (S) + `TUI-044` (S)** — three small edits to
   `editor.rs` that stop the prompt editor silently substituting marker text for a paste. Both
   shipments are band 1 and both are mostly `S`.

**2. Audit what the recent closures shipped, not that they shipped.** *(Raised from position 12 —
   see structural defect **E**, and the critique finding that produced the move.)* Both audited
   provider closures produced highs: `PROV-005` → `PROV-027`/`028`/`029`/`030`. **The named,
   unaudited scope is:** `amazon-bedrock` and `openai-codex`, which arrived in the same sweep as
   `google-vertex` and `github-copilot` with **no read-against-upstream pass at all**; `cf26010`'s
   **other nine OAuth flows** (only the Copilot/Codex halves have been read); and the **~28k lines
   `8902b4f` landed in one commit** (watchdog/, missions/, tui/fleet*), of which only the three
   `PARITY-GAPS` UW holes are documented. Expected yield exceeds most of what was previously ranked
   above it, and the two provider names alone are the same shape that produced `PROV-030`.

**3. The three fail-open security defaults, plus the shell surface.** `PERM-009` (S — delete three
   lines and a comment) · `PERM-023` (S) · `EXT-054` (M) with `EXT-055` (S) in the same edit ·
   `TOOL-039` + `TOOL-007` as **one** shell-surface decision. Band 3. `EXT-054` has zero blast radius
   today, which is exactly why it should land before the first third-party guest rather than after.

**4. `TUI-027` (M).** The remaining item where a mistyped key **writes** persisted data. Needs
   `/tree` text search, the keymap rebind, and the seven `app.tree.filter.*` ids in
   `TreeAction::from_id`. **`CFG-048` must land first** or the namespace rename breaks every
   `editor.*` config written against shipped cyrup.

**5. The pre-launch startup surface — `SEAM-064` (S) + `SEAM-062` (S) + `SEAM-063` (M) + `SEAM-065`
   (M) + `SEAM-061` (M).** Five highs, one surface, all from the `packages/coding-agent/src/cli/`
   sweep that had never been run. `SEAM-064` is a one-character production change. Every one of these
   needs a **live terminal run** to verify, not a driven event loop.

**6. The compaction-safety trio `SESS-040` + `SESS-041` + `SESS-042` (M, one path), then `TUI-031`
   (M).** One afternoon on a path that bills tokens and rewrites the session file. Do `TUI-031`
   immediately after — same subsystem, and it is the reason a cancelled compaction matters.

**7. `SEAM-051` (S).** Band 5, but first within it: the default value of a v0.84.1 flag makes the
   binary exit 1, and it is one token from working. No pi migrant can launch cyrup today.

**8. The provider wire cluster.** `PROV-027` + `PROV-028` in **one** edit (PROV-028 needs the exact
   provider guard PROV-027 introduces — separately is strictly more work), `PROV-029` alongside as
   one field assignment per provider; then `PROV-048` + `PROV-049` + `PROV-050` (the SSE
   surrogate/astral trio, all `S`, all in `json_parse.rs`).

**9. `SUBA-043` (S) + `SUBA-014` (S).** Two head-injections in the same crate, both silent, both
   under a day. Cheapest high-value pair in the backlog.

**10. `PROV-047` (M) + `CFG-035` (M).** Egress that ignores the proxy setting on seven paths; a trust
   gate that asks about a file cyrup will never read.

**11. Signal teardown — `SEAM-047` (M) with `SEAM-059` (S, same function), `SEAM-008` and
   `DRIFT-049`.** Four ids, one defect. Blocks any supervised deployment of `--mode rpc`. **Schedule
   once, in area 08.**

**12. `PROV-030` (L).** A whole wire API, plus the mandatory `all.rs:12-47` doc correction. Correctly
   last among the highs: it fails loudly and immediately.

**13. The WIT/ABI batch — cluster F2, one `cyrup:ext@0.5.0` bump, 27 ids.** Land `TOOL-021` (owned
   `prompt_guidelines`) in the same move; without it `impl Tool for WasmTool` can never carry
   guidelines. `L`-once versus 27 × `M` and 27 guest-refusal cliffs.

**14. The test-defect sweep — cluster F3, 23 ids, one pass.** The suite is at zero failures, so any
   new failure is signal. Deduplicate `AGENT-019` / `DRIFT-039` first.

**15. A third surface-driven sweep, on the axes the area files now name as unrun.** This pass's two
   most productive axes were both new: **cyrup's own asserted invariants, inverted** (grep the claim,
   then grep for a reader — this produced `EXT-054` and `EXT-055`, and a pi-anchored sweep is
   structurally blind to it because pi has no capability model), and **the non-drawing files of an
   upstream package** (`stdin-buffer.ts` is 434 lines that draw nothing, and it produced two
   criticals). The named unrun targets, in the files' own words: **`crates/cyrup-tui/src/editor.rs`
   read line-for-line against `components/editor.ts` @v0.83.0** — `TUI-042`/`043`/`044`/`049` were all
   found from *outside* that file, which strongly implies more inside it (area 07 blind spot 9);
   **`pi-intercom`'s 68 top-level exports at v0.9.2, restricted to `broker/` first** — no pass has
   ever walked that surface, because every axis used there was bounded by the drift window or by what
   a prior pass happened to close (area 11 blind spot 10); and **symbol-by-symbol enumeration of
   `core/extensions/{types,runner,loader,index}.ts`** (area 06 blind spot 8).

---

## Structural defects in this analysis — status update

**A. Split open-items tables hid items from every enumeration. — RETIRED.** Area 03's second table
was the last instance and the repair pass deleted it: `SESS-S05` moved into the main table as a low
row with its id, severity, kind, effort and status untouched, the `## Surface-sweep findings` heading
and body retained for provenance, and an explicit instruction not to re-add a second table.
**All twelve files now have exactly one open-items table**, verified mechanically (one severity-table
header per file; every row has a body and every body has a row). Areas 08, 09 and 12 additionally
carry a separate `## Trackers` table — that is deliberate and is *outside* the count, not a split.

**B. Surface-sweep items carried no verified upstream trace. — RETIRED.**

**C. An item-driven analysis cannot see behaviour nobody wrote an item for. — CONFIRMED, HARD, and
demonstrated twice in one day.** The first pass filed 176 against 117 closures. The repair pass then
read four upstream surfaces nobody had named and filed **31 more, including 2 criticals and 8
highs**, without any new code being written. **Treat 448 as a floor, not a total**, and treat any "we
are N items from done" claim as unsupported.

**D. Files contradict themselves. — the named instances are fixed; a NEW class was found and it is
worse.** Resolved: area 06's `EXT-028` row, area 12's `DRIFT-026` contradiction, area 10's
`PERM-001`/`PERM-005` prose. **Struck as false:** this file's own claim that `PARITY-GAPS.md`'s
header was stale (see the header block). **The new class is code contradicting the analysis:**
`providers/all.rs:12-47`'s port-status table calls four registered providers "pending (NOT
registered)" and contradicts its own table row at `:21`, so an engineer opening `PROV-030` reads a
denial of the item before reading the item. The doc correction is now a mandatory part of
`PROV-030`'s Fix rather than a separate id — a separate id would let the code fix land without it.

**E. A closure reliably ships defects inside the code that closed it. — CONFIRMED, and now a rule.**
`PROV-005`'s closure shipped `PROV-030` **and** `PROV-027`/`028`/`029` · `ICOM-022`'s closure shipped
`ICOM-027`, `ICOM-043` and `ICOM-048` · `ICOM-002`'s closure shipped `ICOM-035` · `AGENT-S01`'s fix
shipped `AGENT-021` and `AGENT-029` · `SESS-023`'s closure is what exposed `SESS-040`/`041`/`042` ·
`TOOL-004`'s fix shipped `TOOL-019` · `EXT-S02`'s closure sharpened `EXT-013`, `EXT-017`, `EXT-053` ·
`SUBA-S01`'s closure left `SUBA-043` (high) · `SUBA-S03`'s closure left `SUBA-051` · `SUBA-007`'s
closure left `SUBA-047`.

  **The rule: closing a "not implemented" item means the subsystem now EXISTS, not that it is
  CORRECT.** A closure is not done until the closing code has been read against upstream. This is
  ranking item 2.

**F. The "advertised but inert" class is mechanically detectable and nobody is detecting it. —
CONFIRMED and growing.** See cluster F1 and ranking item 0.

**G. NEW — citation drift between the two tags is systemic, not incidental, and it reached the #1
item in the backlog.** `AGENT-020` cited `agent.ts:361-388` "(identical at v0.83.0 and v0.84.1)"
when `async continue()` is at `:350` at the ported baseline. The repair pass swept the class and
found **twenty-plus further instances across areas 01, 02, 04 and 07** — nine wrong in area 01 alone,
the worst on a high (`PROV-029` quoted `isSubscription: true` from `github-copilot.ts:16` "@v0.83.0",
a property that **does not exist at v0.83.0 at all**), `PROV-023` cited a line holding a *different
flag*, `PROV-024`'s four cites matched **neither** tag, and `TOOL-036` proved to be half
upstream-drift because `normalizeWindowsShellPath` landed *inside* the window. The `agent-loop.ts`
shift is **not uniform** (0 through `:636`, +4 from `:642` on, because the block arm was rewritten),
which is exactly the hazard `README:224-225` warns about. **Method, now written into three area
files: never write "identical at both tags" — give per-tag offsets, each labelled with the tag it was
read at, and re-resolve by opening the file rather than shifting.** Area 01 proposes widening
`PROV-041`'s citation lint to cover `docs/gap-analysis/*.md`; that is the mechanical fix.

**H. NEW — a commit hash is not evidence of a classification.** Nine area-12 items rested on a hash
rather than a two-sided read. Seven were re-derived cheaply this pass and **six proved
misclassified** (see *Version lag*). The remaining two (`DRIFT-023`, `DRIFT-040`) are now trackers in
an explicit `## Leads — not yet evidenced` section outside the count, each carrying the exact commands
that would settle it. **Rule: `git cat-file -e v<ported-tag>:<path>` must run before any
`upstream-drift` kind is assigned** — a hash answers "when did this land upstream", but the
classification turns on "before or after the tag cyrup was ported from". The generalisation of the
same error: *"no runtime effect" licenses skipping a directory's behaviour, never its provenance — a
gitignored path is evidence that an artifact is generated, hence that a generator exists, not
evidence that the generator is absent.* That sentence is why the catalog generator went unfound for
three editions.

---

## Corrections to carry forward — these are wrong ABOUT THE CODE or ABOUT UPSTREAM

1. **DISCHARGED.** The false `subagent-executor.ts:3022` "pi precedent" is fixed in place; all five
   surviving occurrences in `cyrup-ext-subagents/src/extension.rs` are now the **correction**.
2. **STANDS.** `EXT-S01`'s original Impact was wrong about pi in a way that could have inverted a
   security default. The lesson stands: an item's Fix text is a hypothesis.
3. **STANDS.** `CFG-002`'s prescribed Fix was wrong and the implementer correctly ignored it. pi
   throws unconditionally (`provider-composer.ts:167-169`).
4. **DISCHARGED.** `PROV-026` is struck — `seed.json`, `seed_catalog()` and `seed_catalog_parses` no
   longer exist.
5. **`SEAM-019` is unworkable as written and is superseded by `SEAM-051`.** Its premise
   (`--ui-mode` / `--alt`) is false at **both** tags — pi has never had either flag.
6. **`CFG-021` was misdescribed twice.** The key is `tuiMode`, not `uiMode`; both it and
   `fullscreenScrollbar` are v0.84.1 additions, so its kind is `upstream-drift`.
7. **`DRIFT-028` is halved.** The `~anthropic/*` alias claim is refuted: pi's own runtime detector is
   byte-equivalent to cyrup's. A shared upstream bug is not a cyrup divergence.
8. **`SUBA-024` and `SUBA-021` cite files that never existed.** `chain-validation.ts` has no history
   at any tag; `launch-contract.ts` is absent at both v0.43.0 and v0.47.1.
9. **NEW — `DRIFT-009`'s "no in-tree regeneration source" was false**, and it produced a Fix
   (seeding from the pi.dev artifact) that is strictly lossier than the correct one. Rewritten to
   defer to `PROV-018`. **This ledger repeated the error as "catalog lag is unresolvable from this
   workspace"; both are struck.**
10. **NEW — this ledger asserted twice that `PARITY-GAPS.md`'s header was stale. It was not.**
    `PARITY-GAPS.md:3-8` records v0.47.1 and v0.10.1 and *opens* by explaining the correction. Struck
    from *Version lag* and from structural defect D.
11. **NEW — `TOOL-036` is half upstream-drift.** `normalizeWindowsShellPath` returns nothing at
    v0.83.0; it landed inside the window. The `~`/`os.homedir()` half **is** at v0.83.0 (`paths.ts:67`)
    and remains a genuine baseline parity bug, so the item keeps `kind=parity-bug` with the drift half
    labelled. Root cause: area 04's version-lag sweep was scoped to `core/tools/`, and `utils/paths.ts`
    is not under that path.
12. **NEW — `SEAM-035`…`SEAM-046` never existed.** `git show a9000b1:docs/gap-analysis/08-….md | grep -o 'SEAM-0[34][0-9]'`
    returns 030–034 only, and the missing ids appear in no file. It is a numbering artifact of this
    pass starting its new ids at `SEAM-047`, **not a deletion**. Honest caveat: `docs/gap-analysis`
    has exactly **one** commit in cyrup's history (`a9000b1`), so an id dropped before the directory
    came under source control is invisible to that check — to this pass and to every pass.

---

## By area

> **SUPERSEDED 2026-08-14** — see the *By area* table at the top of this file.

Highs, mediums **and** lows have all been audited against code at HEAD `04c1ba2`. Counts are derived
from each file's own single `## Open items` table; the `trk` column is excluded from every other
column and from the total.

| file | open | crit | high | medium | low | trk | closed | filed | open criticals + highs |
|---|---|---|---|---|---|---|---|---|---|
| [01-cyrup-core-and-provider](01-cyrup-core-and-provider.md) | 40 | 0 | 6 | 14 | 20 | 1 | 10 | 22 | `PROV-027` `PROV-028` `PROV-029` `PROV-030` `PROV-047` `PROV-048` |
| [02-cyrup-agent](02-cyrup-agent.md) | 26 | 1 | 1 | 6 | 18 | 1 | 3 | 14 | **`AGENT-020`** `AGENT-030` |
| [03-cyrup-session](03-cyrup-session.md) | 29 | 0 | 1 | 13 | 15 | 1 | 9 | 9 | `SESS-040` |
| [04-cyrup-tools](04-cyrup-tools.md) | 29 | 0 | 1 | 10 | 18 | 0 | 14 | 11 | `TOOL-039` |
| [05-cyrup-config-and-resources](05-cyrup-config-and-resources.md) | 38 | 0 | 1 | 19 | 18 | 0 | 16 | 17 | `CFG-035` |
| [06-cyrup-ext](06-cyrup-ext.md) | 50 | 1 | 0 | 28 | 21 | 0 | 6 | 21 | **`EXT-054`** |
| [07-cyrup-tui](07-cyrup-tui.md) | 56 | 3 | 1 | 26 | 26 | 0 | 13 | 25 | **`TUI-027`** **`TUI-042`** **`TUI-043`** `TUI-031` |
| [08-cyrup-session-svc-and-modes](08-cyrup-session-svc-and-modes.md) | 40 | 0 | 7 | 19 | 14 | 1 | 6 | 24 | `SEAM-047` `SEAM-051` `SEAM-061` `SEAM-062` `SEAM-063` `SEAM-064` `SEAM-065` |
| [09-cyrup-ext-subagents](09-cyrup-ext-subagents.md) | 45 | 0 | 2 | 23 | 20 | 1 | 22 | 24 | `SUBA-014` `SUBA-043` |
| [10-cyrup-permission-system](10-cyrup-permission-system.md) | 21 | 1 | 1 | 6 | 13 | 0 | 10 | 7 | **`PERM-009`** `PERM-023` |
| [11-cyrup-intercom](11-cyrup-intercom.md) | 44 | 0 | 0 | 22 | 22 | 0 | 3 | 24 | — |
| [12-upstream-drift-pi-core](12-upstream-drift-pi-core.md) | 30 | 0 | 1 | 11 | 18 | 4 | 5 | 9 | `DRIFT-049` *(dup of `SEAM-047`)* |
| **total** | **448** | **6** | **22** | **197** | **223** | **9** | **117** | **207** | |

**Where the backlog actually sits.** Areas **07 (56)** and **06 (50)** carry 24% of it between them,
and 06's 28 mediums are still the largest single medium block anywhere. **Area 08 is the new
concentration of *severity*** — seven open highs, more than the next three areas combined, and every
one of them from a single unrun axis. Area **11 (44)** is third by volume and is almost entirely new,
because its true baseline was only just established; its zero highs should be read as *un-swept*, not
clean.

---

# Archive — superseded provenance

> **DO NOT PLAN FROM THIS SECTION.** Everything below is retained as the historical record required
> by the stable-id rule. It was true when written and is **not** true at `04c1ba2` after the repair
> pass. Read it for *how the analysis got here*, not for what to do.

### Archived — the first 2026-08-12 edition's headline claims (superseded the same day)

> Its three headline results were: **(1)** "Zero critical items and zero pre-2026-08-11 highs remain
> open" — **superseded**, an artifact of not applying `README.md:106-107`'s own definition; there are
> six criticals, four of which were already in that edition's backlog, mis-rated. **(2)** "The medium
> closure rate went from 3.9% to roughly 29%" — **stands**, and the analysis behind it stands; it is
> reproduced above. **(3)** "The open count went UP anyway: ~311 → 426; treat 426 as a floor" —
> **superseded by 448 raw / ~420 deduplicated**, and the reason it was superseded is that 426 was
> simultaneously called a floor and inflated by ≥25 ids of documented double-count. Both figures are
> now published separately.
>
> Its 14-row actionable table ranked `AGENT-020` first (correct, and the item is now critical),
> `TUI-027` second (now critical), `EXT-054` seventh (now critical) and did not contain `PERM-009`,
> `TOOL-039`, `SEAM-051` or any of `SEAM-061`…`SEAM-065` at all — the first three because they were
> mis-rated, the last five because the axis that found them had not been run. Its ranking put "audit
> what the recent closures shipped" at **position 12**; it is now position 2.
>
> Two of its statements were false about other files and are struck in the corrections list above:
> that `PARITY-GAPS.md`'s header was stale, and that catalog lag is unresolvable from this workspace.

> **UPDATE — GitHub Copilot, 2026-08-11, cyrup HEAD `097bdde`.** `PROV-005` (four providers
> unimplemented) is **CLOSED** — `amazon-bedrock`, `openai-codex`, `google-vertex` and
> `github-copilot` are all constructed in `providers/all.rs:177-197`, and `register_builtins`
> (`api/mod.rs:131-163`) now registers 9 factories including the formerly dangling
> `bedrock-converse-stream`. **But the Copilot port that closed it carries three new highs**, filed
> as `PROV-027`/`028`/`029`:
>
> * **PROV-027** (parity-bug, S) — Copilot's 9 Claude models send `x-api-key`; pi has an explicit
>   Copilot branch sending `Authorization: Bearer`.
> * **PROV-028** (not-ported, S) — `github-copilot-headers.ts` has no counterpart, so `X-Initiator`,
>   `Openai-Intent` and `Copilot-Vision-Request` are absent on **all three** wire routes.
> * **PROV-029** (parity-bug, S) — the Copilot **and** Codex login flows are fully ported and
>   unreachable.
>
> Two things generalise past Copilot. **(1) A "not implemented" item closing means the subsystem now
> exists, not that it is right.** **(2) `PROV-003`'s "deprioritised, out of scope" status was stale**:
> `cf26010` landed all 11 OAuth flows under `auth/oauth/`.

> **UPDATE `6104dfa` + `7fd0d9c` (2026-08-07).** `control` and `includeProgress` are ported for real
> and live in the schema. `SUBA-N04` closed (the field was dropped in THREE places, not one).
> `TOOL-019` closed and revert-proved twice. `PROV-006` closed as a process-global **idle** timeout
> mirroring pi's `configureHttpDispatcher` — deliberately not a total-request deadline, which would
> kill a healthy model streaming a token every 20 s. `PROV-008` capped at pi's 4000 chars.
> **3932 passed / 0 failed / 8 ignored across 225 suites.**

> **REWRITTEN 2026-08-07 after a code-level audit of every high/critical item at HEAD `8d00f06`.**
> The previous version of this file was baselined at `1806375`, **eight commits behind**, and an
> auditor called it *"the most dangerous file in the directory"* — a planner reading it would have
> scheduled about twelve already-completed items.

### Archived — "The real number: 4 actionable highs, not 17 — now 7" (2026-08-07 / 2026-08-11)

38 high/critical items were audited on 2026-08-07. **31 CLOSED, 5 OPEN, 2 PARTIAL.**

| ID | area | state *(as of 2026-08-11)* | why it mattered |
|---|---|---|---|
| **SUBA-S01** | 09 | OPEN — **now CLOSED** | A declared `outputSchema` never reached the child in any form. *(Its residual is `SUBA-043`, still high.)* |
| **SUBA-N03** | 09 | **CLOSED** | Overrides refused on the async/background branch. ~~**EIGHT params, not seven.**~~ **CORRECTED 2026-08-14 (sweep 6): the closure covers ELEVEN params** — output, outputMode, skill, share, sessionDir, artifacts, acceptance, control, includeProgress, timeoutMs, maxRuntimeMs — each asserted (a) not to hit the refusal, (b) to reach agent resolution, (c) to be an advertised property; **and the load-bearing half is separately pinned: two companion tests assert the params reach the DETACHED hop-2 runner at the `runner-config.json` boundary, not merely past the router.** Do not re-open it with the eight-param framing. |
| **EXT-S02** | 06 | OPEN — **now CLOSED** | Extension slash commands never reached the TUI `/` autocomplete. `SlashCommand::name` is `Cow<'static, str>` at `commands.rs:36`. |
| **TUI-S01** | 07 | PARTIAL — **now CLOSED as framed** | 6 of 9 `UiEffect` mutators wired. Residue is `TUI-014` + `TUI-033`. |
| **PROV-027 / 028 / 029** | 01 | OPEN (2026-08-11) — **still open** | See the Copilot update above. All three remain in the current actionable table. |

**Archived deferral note (2026-08-07/11), now superseded:** *"Deliberately out of scope: `CFG-005` —
the OAuth acquisition cluster… Also deferred as subsystems: `steer`, the four `schedule*` verbs,
`watchdog/`, FleetView."* — **three of those four subsystem deferrals are stale**, and the `schedule*`
count was nine, not four; see *Deliberately out of scope* above.

### Archived — "The mediums were audited too — 169 genuinely open" (2026-08-07)

> **Medium audit, 2026-08-07, HEAD `8d00f06`. 178 items read against code. 166 OPEN, 3 PARTIAL,
> 7 CLOSED, 2 WRONG. Closure rate 3.9%** — against **82%** for the highs.

The mechanism is plain once stated: every commit since the analysis was written was *explicitly*
high-targeted (`513e45a`'s own message: "close the 8 remaining **high-severity** gap items"). **All 7
closures were collateral of a high fix**, never independent work. `git log 1806375..HEAD` returned
**zero** commits over `retry.rs`, `compat.rs`, `cyrup-core/src/message.rs`, `env_keys.rs`, `hooks.rs`,
`openai_completions.rs`. Seven whole areas came back **100% open**.

*(Superseded 2026-08-12: the medium rate is ~29% and no area came back 100% open. The 3.9% figure and
its diagnosis are retained because the diagnosis was correct and is what predicted the change — the
rate moved when the commits stopped being item-targeted.)*

### Archived — promoted medium → high (2026-08-07)

| ID | promoted because | status at `04c1ba2` |
|---|---|---|
| `TOOL-019` | A regression one of OUR OWN fixes introduced: `cfe351e`'s `TOOL-004` fix moved to `write_in_place`, so an unlocked cross-registry race interleaved bytes. | **CLOSED** (`7fd0d9c`, process-global `static LazyLock`, revert-proved). Residual `TOOL-032`. |
| `PROV-006` | **No request timeout at ANY layer**, and a silent no-op on a shipped control. | **CLOSED.** Real retry module + idle timeout, seven api impls. Residual `PROV-043`. |
| `PROV-010` / `AGENT-014` / `DRIFT-012` | One defect, three ids: a truncated stream transcribed as a cleanly completed turn and persisted to JSONL. | **ALL THREE CLOSED.** `StopReason::Pending`+`Deferred`+`raw_stop_reason`, verified across all six producer sites. |
| `SUBA-S06` | `drive_attempt`'s `tokio::select!` had no `child.wait()` arm — a child exiting without closing inherited stdio hung the orchestrator forever. | **CLOSED** (`exec/mod.rs:2826-2856`). |
| `TUI-N02` | `/reload` silently fails the permission gate **OPEN**. | **STILL OPEN** (medium, area 07). |

### Archived — clusters as stated on 2026-08-07

**C5 — "the extension ABI is lossy; batch every WIT change into ONE bump" — 15 ids.** *(Superseded:
the world reached `@0.4.0`; the cluster is now 27 ids against a `0.5.0` bump — see F2. The
`prompt_guidelines` prerequisite is unchanged and still unbuilt.)*

**C10 — the subagent second-hop config boundary** — `SUBA-006/007/008/014` plus `SUBA-N03/N04/N05/N06`.
*(Superseded: all four `-N` residuals closed; `SUBA-006` closed; `SUBA-007` partially closed;
`SUBA-014` is still open and is now **high**.)*

**C2 — no HTTP timeout or retry budget.** *(Superseded: closed, residual `PROV-043`.)*

### Archived — open work that appeared in no high row (2026-08-07)

- `includeProgress` and `control` were UNTRACKED and de-advertised. *(Closed — `SUBA-N06`.)*
- `SUBA-N04` filed medium, should have been high. *(Closed.)*
- `chainDir` advertised and dead. *(Closed — `SUBA-N05`.)*
- The schema is `additionalProperties: true` — 30 advertised, 32 parsed. *(The class survives as
  cluster F1 and as `SUBA-043`/`SUBA-047`.)*
- The `PROV-007` closure residual: `registry_models()` binds to `builtin_catalog()` (pi's
  credential-blind `getModels()`) where pi's six call sites use `getAvailable()`. *(Survives as
  `PROV-031` — and the `cli/` sweep found the same defect a second time on the `--list-models` path,
  which is why `SEAM-020` was re-rated low → medium.)*
