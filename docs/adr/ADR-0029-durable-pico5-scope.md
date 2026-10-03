# ADR-0029 — Pico5 has a specification; cyrup builds to the contract, not to the code, and takes the durability kernel only

**Status** accepted (decided by default under the parity rule — overridable)
**Date** 2026-10-02
**Decides** the scope question for pi's `packages/durable` / Pico5 at `v1.0.0`, raised by area 17 (`docs/gap-analysis/17-pi-harness-and-durable.md`) and left open there as six watch-only trackers. Re-decides the premise of `docs/adr/ADR-0004-agent-harness-scope.md`, whose subject pi deleted.
**Blocks released** area 17's `HARN-002` and `HARN-004` gain a named owner, so no later pass re-derives the sizing argument; `docs/PICO5-PLAN.md` becomes schedulable; `docs/adr/ADR-0030-durable-rust-architecture.md` gets its scope boundary. ADR-0004's automatic-reversal tripwire is replaced here, because the path it watches no longer exists.

---

## Context

### The question, and why ADR-0004 does not already answer it

ADR-0004 decided OQ-2 — *is pi's agent harness in scope?* — and answered **port the behaviour the
harness pins, not the harness**, on a measurement: `packages/agent/src/harness/**` was a published
SDK rewrite that pi's own binary did not consume, so it contributed zero user-visible behaviour to
be 1:1 with.

**pi v1.0.0 deleted that subject.** `git -C tmp/pi ls-tree -r --name-only v1.0.0
packages/agent/src/harness/` returns **0 entries**, and `packages/agent/CHANGELOG.md:6` @v1.0.0
declares the removal a Breaking Change naming *"sessions and session storage, the durable runtime,
pico3, harness tools, compaction, skills, prompt templates, system prompt helpers, telemetry
schemas"* and directs consumers to `@earendil-works/pi-durable`.

So ADR-0004's holding is intact and its subject is gone. The harness moved, grew by an order of
magnitude, and acquired something ADR-0004's subject never had: **a 4 601-line specification**. That
last fact is new, material, and the reason this is a re-decision rather than a re-statement.

Every measurement below was taken from `git`-object reads at the named tag. No working tree was
modified in either repository.

---

### Measurement 1 — the subject, re-derived

```
$ git -C tmp/pi ls-tree -r --name-only v0.87.1 packages/durable/src | wc -l
8
$ git -C tmp/pi ls-tree -r --name-only v1.0.0   packages/durable/src | wc -l
60
```

Summed with `git show v1.0.0:<path> | wc -l` over every file:

| | v0.87.1 | v1.0.0 |
|---|---|---|
| `src/**` files | 8 | **60** |
| `src/**` lines | 2 087 | **17 662** |
| `docs/spec.md` | — | **4 601** |
| `test/**` files | — | 83 |

**A correction to area 17, offered for the record.** `17-pi-harness-and-durable.md:133` and
`HARN-004`'s body both state *"~18 600 lines"*. The measured figure is **17 662**. The difference is
not material to any decision, but the number should be the measured one, and area 17's is the one
that will be carried forward unless it is corrected here.

**A second correction, which is material to ADR-0030's invariant map.** `17-pi-harness-and-durable.md:133`
and `HARN-004`'s body both say *"five task kinds"*. There are **three**:

```
$ git -C tmp/pi grep -n 'name: "pi\.' v1.0.0 -- packages/durable/src/
packages/durable/src/harness/compaction.ts:103:  name: "pi.compaction",
packages/durable/src/harness/generation.ts:116:  name: "pi.generation",
packages/durable/src/harness/tool.ts:51:      name: "pi.tool",
```

and `spec.md:3168-3176` (§8 *Built-in tasks*) is a three-row table. The number five belongs to
`TaskState`, which has exactly five statuses — `pending`, `running`, `waiting`, `completing`,
`terminal` (`spec.md:1580-1594`). The conflation matters because the closed set a cyrup enum would
model is five *states* and three *kinds*, and a design that mints five kind variants would be
modelling the wrong closed set.

### Measurement 2 — the subject splits almost exactly in half, along the seam that decides scope

Per-subtree, `src/**` at v1.0.0:

| subtree | lines | what it is |
|---|---|---|
| `src/storage/**` | 2 926 | three backends + the storage contract (§10, §11) |
| `src/session/**` | 1 989 | `session.ts` 568, `transaction.ts` 1 023, `observation.ts` 301, `forks.ts` 97 (§3.4, §4, §9) |
| `src/types.ts` | 1 085 | the record set, ids, queries (§2) |
| `src/{documents,truncate,index,entries,errors,ids,tasks}.ts` | 658 | record helpers and re-exports |
| **durability kernel subtotal** | **6 658** | **§1–§4 + §10–§11** |
| `src/testing/**` | 2 179 | the exported conformance / reopen / benchmark suites (§10:4369) |
| `src/harness/**` | 6 606 | scheduler 1 337, generation 677, tool 488, compaction 451, harness 433, events 425, … (§5–§9) |
| `src/env/**` | 1 131 | its own `ExecutionEnv` |
| `src/tools/**` | 1 088 | its own `CodingTools` — bash, read, write, edit, edit-diff, image |
| **harness subtotal** | **8 825** | **§5–§9** |

The specification splits the same way: §1–§4 is 1 522 lines, §10–§13 is ~410, and §5–§9 is **2 179**.

This is the seam. **`src/harness/**`, `src/env/**` and `src/tools/**` — 8 825 lines, half the
subject — are a second agent loop, a second execution environment and a second coding toolset.**
cyrup already ships all three: `crates/cyrup-agent/src/agent/run/`, `crates/cyrup-session-svc`, and
`crates/cyrup-tools` (whose `powershell.rs` is a capability pi's set does not have). ADR-0004's
measured conclusion — *"'absorb the harness' would not fill a hole; it would add a second, unused
session/tool/compaction stack alongside the one that runs"* (`ADR-0004:198`) — applies to this
subtree verbatim, against a larger number than it was written for.

The other half is different in kind. `src/storage/**` plus `src/session/**` plus `src/types.ts` is a
durability contract cyrup has **no counterpart to at all**, and its guarantees are what every claim
in pi's own README rests on: *"Kill the process in the middle of a tool call and start it again with
`--continue`: the interrupted call gets an interrupted result and the turn finishes."*

### Measurement 3 — pi still ships none of it, and the evidence is now stronger than an import trace

Area 17 established this at the import level. At v1.0.0 pi's own **package manifest** states it:

```
$ git -C tmp/pi show v1.0.0:packages/coding-agent/package.json | sed -n '29,34p'
  "files": [
    "dist",
    "!dist/client",
    "!dist/experimental",
    "!dist/cli/experimental",
```

Every consumer of `pi-durable` and of chord lives under `packages/coding-agent/src/experimental/**`
(`client-runtime.ts`, `client-tui.ts`, `client.ts`, `experimental/durable/{harness-setup,runtime,subagent}.ts`,
`experimental/plugins/{bundled,package}.ts`, `server.ts`,
`experimental/services/agent-controller-provider.ts`). Those compile to `dist/experimental`, which
pi's `"files"` array **excludes from the published tarball by name**. And:

```
$ git -C tmp/pi grep -n "experimental/" v1.0.0 -- 'packages/coding-agent/src/*' ':!packages/coding-agent/src/experimental/*'
(no output)
```

No non-experimental source file references the directory at all. `packages/coding-agent/src/cli.ts`
imports exactly two things (`./cli/setup.ts`, `./main.ts`). `@earendil-works/pi-durable` is **not a
declared dependency** of `coding-agent`.

**One honest refinement to area 17's `DUR-001`.** `@earendil-works/chord` *is* a declared dependency
of `coding-agent` — `package.json:52`, `"^1.0.0"` at v1.0.0 and `"^0.87.1"` at v0.87.1, so the edge
is not new. It is consumed only from `src/experimental/**`, which the `"files"` exclusion drops.
`DUR-001`'s sentence *"no shipped `coding-agent/src` file imports chord"* is true as written; the
declared-dependency line is a stronger thing to watch than the import, so this ADR's tripwire watches
both.

### Measurement 4 — chord is still moving, and still disclaims its own contract

```
$ git -C tmp/pi diff --shortstat v0.87.1 v1.0.0 -- packages/chord/
 49 files changed, 10394 insertions(+), 4995 deletions(-)
$ git -C tmp/pi ls-tree -r --name-only v0.87.1 packages/chord/src/state | wc -l
3
$ git -C tmp/pi ls-tree -r --name-only v1.0.0   packages/chord/src/state | wc -l
0
```

`src/state/{diff,draft,value}.ts` (997 lines) are gone; `src/delta/` is canonical. And
`packages/chord/PLANNING.md:3` at v1.0.0 still reads, verbatim:

> This is not a stable public API contract yet.

`packages/durable` has a hard dependency on that package. **Porting durable's implementation now is
a bet on re-porting it**, and the measured churn rate says the bet loses.

### Measurement 5 — the fact that changes the answer

`packages/durable/docs/spec.md` is **4 601 lines, 13 sections**, and it is not a design sketch. It
carries:

- **§1, eight numbered required invariants** (`spec.md:63-78`) — atomic commit, publish-after-commit,
  no volatile path, no effects in the transaction, immutable never-reused ids, full draft revocation
  with copied strict-JSON values, the mutation line held through adoption, uncertain failure fatal.
- **§10, a storage contract** (`spec.md:4188-4369`) stated as obligations on a backend, ending
  (`:4369`) by naming an executable semantic conformance suite — *not a type* — as the contract.
- **§12, thirty-four API footguns** (`spec.md:4450-4585`), opening with *"These are contracts, not
  invitations to add defensive machinery."*
- **§13, eleven explicit non-goals** (`spec.md:4587-4601`).

A specification is a far more stable artefact than the implementation beneath it, and it is a
different kind of thing to port. **An implementation port of a package whose dependency churned
+10 394/−4 995 in ten days is a liability. A design built to a 4 601-line contract is not** — the
contract is what the churn is converging *toward*, and §1's invariants and §13's non-goals are the
parts least likely to move, because they are the statements the implementation is measured against.

This is the whole reason the answer here differs from ADR-0004's. ADR-0004 faced an implementation
with no contract and no consumer, and correctly declined it. This decision faces a contract.

---

## Decision

**Design to the Pico5 specification now. Build the durability kernel — §1–§4 and §10–§11 — as
Rust-native cyrup code. Do not port the harness (§5–§9), and do not port any implementation.**

Concretely, an implementer does the following and re-derives nothing:

**1. Adopt Pico5's guarantees as cyrup's durability contract of record.** The eight §1 invariants and
the §10 storage obligations become cyrup's, restated as guarantee handles and mapped to Rust
mechanisms in `docs/adr/ADR-0030-durable-rust-architecture.md`. Full capability parity with the
guarantees; **no behavioural concession**.

**2. Build the kernel, in the slices of `docs/PICO5-PLAN.md`.** Identity and value types; the pure
document core (Chord's seven operation tuples, base-plus-delta replay, checkpoint selection, version
migration); the storage contract trait with a keyed batch and a two-class failure outcome; a memory
backend as the normative reference; the session mutation line, transactions and drafts; observation
and publication; a JSONL-shaped file backend with an exclusive store lock; and cyrup's own
conformance, reopen and fault-injection suites.

**3. Do not build §5–§9.** No scheduler, no five-state durable task machine, no submissions/inbox, no
extension/hook/tool registry, no `pi.generation`/`pi.tool`/`pi.compaction` task definitions, no
`ExecutionEnv`, no `CodingTools`, no conversation view or task-graph view mounts. Those 8 825 lines
duplicate `cyrup-agent`, `cyrup-session-svc` and `cyrup-tools`. **The kernel is designed so they can
be added on top later without reshaping it** — that is what the keyed batch, the two-class outcome
and the one-mutation-line ownership exist to make possible — but adding them is not this decision.

**4. Do not port the implementation, in either half.** The deliverable is Rust-native. Where pi
spends runtime machinery simulating a guarantee Rust gets from ownership — `memory.ts:92-100`'s
recursive `clone`, whose own comment at `:216` says it exists *"to match the ownership boundary"*
SQLite and JSONL get free from encode/decode; Chord's revocation proxy; the `#sealed` flag checked on
every `Tx` method; the `ReadAfterWrite` error class (`errors.ts:4-9`) — that machinery is **deleted,
not translated**. ADR-0030 states each such deletion as a `CYRUP-DELTA`: a mechanism difference at
full feature parity.

**5. Decide the storage engine later, against a number.** cyrup takes **no embedded database
dependency now** (`grep -rE '^(sqlx|rusqlite|redb|sled|libsql) *=' Cargo.toml crates/*/Cargo.toml`
→ nothing today). The storage *trait* is the irreversible decision and lands first; which engine
sits behind it is reversible and is gated on a measured trigger stated in ADR-0030 §Storage. §11's own
framing licenses this: backends differ only in *"durability envelope and performance, and those
differences are stated, not emergent"*, and pi ships JSONL as a peer backend, not a stopgap.

**6. Fix one latent bug in the existing tree, independently of all of the above.**
`crates/cyrup-session/src/store.rs:322-324` does `f.sync_data()?` then `std::fs::rename(&tmp, &self.path)?`
with **no parent-directory fsync**, so the rename itself is not durable on unix. That is a defect
today, not a Pico5 requirement; it is slice S11 in the plan and does not depend on anything else here.

### What would change this decision

Stated as conditions, each mechanically checkable at a new tag:

| # | condition | effect |
|---|---|---|
| T1 | `git -C tmp/pi grep -n 'durable' v<tag> -- packages/coding-agent/src/cli.ts packages/coding-agent/src/main.ts packages/coding-agent/src/cli/` is **non-empty** | pi ships it. §5–§9 re-enter scope; this ADR is superseded. |
| T2 | `@earendil-works/pi-durable` appears in `packages/coding-agent/package.json`'s `dependencies` | the strongest **leading** signal — it precedes T1. Re-read §5–§9 against `cyrup-agent` and re-decide. |
| T3 | `!dist/experimental` leaves `packages/coding-agent/package.json`'s `"files"` array | the consumers become published artefacts. Equivalent to T1 in effect. |
| T4 | `packages/chord/PLANNING.md` drops *"not a stable public API contract yet"* **and** the delta operation set is unchanged across one further tag | the implementation becomes portable; the case for an implementation port can be re-argued. Does not by itself change scope. |
| T5 | a measured cyrup session exceeds the ADR-0030 storage budget (open at p95, or resident index) | the engine decision fires. Does **not** reopen scope. |
| T6 | `spec.md` §1 or §13 changes | the contract moved. Re-read ADR-0030's invariant map; scope is unaffected unless an invariant is withdrawn. |

T1 and T3 are the reversal conditions. T2 is the one to watch, because it fires first.

---

## Consequences

### What this does to ADR-0004

**Amend, with a narrowed subject and a transferred holding. Not superseded.**

ADR-0004's measurement and its holding are both sound, and nothing here contradicts either. Three
things change:

1. **Its subject is narrowed to the deleted artefact.** ADR-0004 decides
   `packages/agent/src/harness/**`, which existed when it was written and does not now. It remains
   the decision of record for that path, and `HARN-003` is the upstream-drift row that records the
   deletion. Nothing in ADR-0004 needs rewriting; it needs a terminal date, which is v1.0.0.
2. **Its holding transfers, and this ADR says so explicitly.** *"Port the behaviour the harness pins,
   not the harness"* is re-applied, against a 3.1× larger subtree (8 825 lines versus ADR-0004's
   measured 9 824 for the whole of harness-v2, of which the comparable portion was smaller), to
   durable's §5–§9. The reasoning is ADR-0004's own and is not re-derived.
3. **Its automatic-reversal tripwire can never fire again, and is replaced here.**
   `docs/adr/README.md` records that ADR-0004 *"reverses automatically on evidence — when its
   tripwire fires (pi's `coding-agent/src` importing `AgentHarness`)"*. `AgentHarness` no longer
   exists in pi; the symbol is unimportable and the grep is permanently negative. **A permanently
   negative tripwire is worse than none**, because a later pass reads it as a live check that keeps
   returning safe. T1–T3 above replace it. This is the single most important consequence of this
   ADR for anyone maintaining the watch list.

### What this does to area 17's trackers

**No gap-analysis rows are filed and no severity changes.** Area 17's seven rows stay exactly as
they are. What changes is that four of them now have a named owner, so the sizing argument is not
re-derived a third time:

| row | disposition under this ADR |
|---|---|
| `HARN-001` (tracker, Pico3 kernel) | subject deleted at v1.0.0 per `HARN-003`. Unchanged. Pico3 is explicitly out of scope — §13:4599 declines a compatibility layer for removed Pico prototypes, so cyrup owes nothing. |
| `HARN-002` (tracker, `pi-durable` has no cyrup counterpart) | **owner: this ADR.** Stays a tracker; its escalation condition (a `pi-durable` import outside `packages/durable` and outside `experimental/`) is T1/T2 restated and is unchanged. Its *premise* was already corrected by `HARN-004`. |
| `HARN-003` (tracker, the deletion) | unchanged. This ADR is what its deletion finding was waiting for. |
| `HARN-004` (tracker, durable became the harness) | **owner: this ADR.** Stays a tracker. Two figures in its body are corrected above (17 662 lines, three task kinds); the correction is recorded here rather than by editing the row, per the convention that an area row is not silently amended. |
| `DUR-001` (tracker, chord 1.0.0) | unchanged; `EXT-088` stays the owner. Refined above: chord *is* a declared `coding-agent` dependency and has been since v0.87.1, which is why T2/T3 watch the manifest rather than the import. |
| `DUR-002` (tracker, experimental client/server re-founded on durable) | unchanged; `SEAM-058` stays the owner. Reinforced: the `"files"` exclusion is a second, stronger reason its trigger has not fired. |
| `DUR-003` (low, dead `pico-v5.md` path) | unchanged and still the one severity-bearing row. This ADR and ADR-0030 cite `docs/spec.md` with section and line anchors throughout, which is what `DUR-003` asked for. |
| `DUR-004` (tracker, release-post packaging claim) | unaffected. |

### What lands, and what it costs

`docs/PICO5-PLAN.md` carries the work as **twelve vertical slices**, one of them a de-risking spike
that runs first and one of them conditional on T5. The kernel is two new crates
(`cyrup-pico-doc`, `cyrup-pico`) plus one backend crate (`cyrup-pico-store-jsonl`); `cyrup-session`
is **not** extended in place, for the reason ADR-0030 §Crate layout states (its `SessionStore` trait
is one line at a time, with no batch, no marker and no confirmed/unconfirmed distinction).

The honest cost: this is a new subsystem of meaningful size with no user-visible behaviour on day
one, built against an upstream contract pi does not yet ship to anyone. The counter-argument is the
one that decided it — cyrup's current answer to *"kill the process mid-turn"* is that the turn is
lost (`crates/cyrup-agent/src/agent/run/` is a process-local loop over an in-memory `RunCtx`), and
that is the gap, not the line count.

---

## Rejected alternatives

**(a) Port the whole of `packages/durable` now, both halves.** 17 662 lines plus 2 179 lines of
conformance suite, against a hard dependency that churned +10 394/−4 995 across 49 files in ten days
and deleted its entire `src/state/` layer. Rejected on the chord measurement first and on
ADR-0004's duplication argument second: §5–§9 would add a second agent loop, a second execution
environment and a second coding toolset beside the three cyrup already runs. Nothing in it is
user-visible in pi, and `"files"` excludes its only consumers from the tarball.

**(b) Nothing now; stay watch-only, as area 17 left it.** This was the right answer at v0.87.1, when
the subject was 2 087 lines of record contracts with zero consumers and no specification. It is the
wrong answer now for one reason: the specification exists. Holding until pi ships means designing
later against whatever the implementation has drifted into, instead of now against a contract — and
it leaves cyrup's own crash-mid-turn behaviour unimproved for as long as the watch runs. Rejected,
but note this is the closest alternative and the one a maintainer is most likely to prefer; the
reversal section says how to take it.

**(c) Interop only — read sessions pi's durable harness wrote.** pi writes
`~/.pi/agent/experimental/durable-sessions/<cwd-hash>/<session>/session.sqlite` under a
`proper-lockfile` lock. Reading it would mean adopting a SQLite dependency *and* pi's schema — the
most expensive dependency with the least benefit, for a format §13:4599 explicitly declines to keep
compatible and whose only writer is an unshipped frontend. Rejected. It is also the option ADR-0004
rejected for the same shape of reason.

**(d) Port the 2 179-line conformance suite first and build to it.** Superficially attractive, since
§10:4369 makes the suite the contract. Rejected because the suite is written against pi's
`interface Storage` — a flat `readonly StorageWrite[]` batch and a structurally-typed `Cursor` bag —
and ADR-0030's central finding is that **that shape is what makes five illegal states expressible**
and forces the same hand-written validation into all three backends. Porting the suite first would
pin the wrong interface. cyrup writes its own conformance suite against its own trait, slice by
slice, and uses pi's as a checklist of *semantics* rather than as code.

**(e) Build §5–§9 too, but thinly.** A thin scheduler and a thin task machine is still a second agent
loop, and the thinner it is the more certainly it diverges from `cyrup-agent`'s real behaviour. If
the durable task machine is ever wanted, the right move is to put `cyrup-agent`'s existing loop
*on* the kernel, not to grow a parallel one beside it. Rejected now; recorded as the shape the
kernel is designed to admit.

---

## How to reverse this

**The sentence.** *"Port durable whole, including the harness"* — or, for the opposite direction,
*"Hold until pi ships it."*

**What would have to change in the tree.** For the first: `docs/PICO5-PLAN.md` gains slices for §5–§9
and an owner for the collision with `cyrup-agent`'s loop, and someone decides which of the two loops
is authoritative — that decision, not the port, is the expensive part. For the second: the plan is
shelved, T1–T3 go on the tag-event checklist in `docs/adr/ADR-0006-upstream-chase-cadence.md`'s
cadence, and `HARN-002`/`HARN-004` keep their current wording with this ADR's two corrections
applied.

**The cost of the option that was rejected.** Option (a) is roughly 8 825 lines of harness built
against a dependency with a disclaimed contract, duplicating three shipped cyrup crates, to deliver
behaviour no pi user can reach. Option (b) costs nothing today and costs the design later: the
4 601-line contract is the most stable thing this subject will ever offer, and a port begun after pi
ships is a port begun against code instead.

**Reverses automatically on evidence.** T1 or T3 firing supersedes this ADR's scope decision without
discussion. T2 firing requires a re-read, not a reversal. T5 fires the engine decision only.
ADR-0030's design survives all five — it is built to the contract, and the contract is what T1–T4
do not change.
