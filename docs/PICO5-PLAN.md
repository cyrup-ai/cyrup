# PICO5-PLAN — the Pico5 durability kernel as landable slices

**Scope** is set by [`adr/ADR-0029-durable-pico5-scope.md`](adr/ADR-0029-durable-pico5-scope.md):
build pi `v1.0.0`'s `packages/durable` §1–§4 and §10–§11 — the durability kernel — as Rust-native
cyrup code, designed to the specification rather than ported from the implementation. §5–§9 (the
harness: scheduler, task machine, submissions, extensions, hooks, built-in tasks, `ExecutionEnv`,
`CodingTools`) are **out of scope**; they duplicate `cyrup-agent`, `cyrup-session-svc` and
`cyrup-tools`.

**Design** is set by [`adr/ADR-0030-durable-rust-architecture.md`](adr/ADR-0030-durable-rust-architecture.md).
Every slice below names the findings (F1–F6) it implements and the guarantees it discharges. The
guarantee handles are the ones in ADR-0030 §2's invariant map, abbreviated: `G-INV-1` … `G-INV-8`
for §1's eight required invariants, and `G-<NAME>` for the §2/§3/§4/§10/§11 obligations.

**Upstream** is read only with `git -C tmp/pi show v1.0.0:<path>`. Cite `packages/durable/docs/spec.md`
by section and line.

---

## Ordering principle

**The riskiest architectural commitment is validated first.** That commitment is not the storage
engine and not the crate layout — it is ADR-0030 F2's commit-callback signature,
`for<'tx> AsyncFnOnce(Tx<'tx, Reading>) -> Result<(R, Tx<'tx, Writing>), _>`. Three guarantees
(`G-INV-4`, `G-INV-6`, `G-READ-BEFORE-FIRST-TABLE-WRITE`) and the deletion of four pi runtime
mechanisms rest on it, and it has not been written against a real borrow checker. If it does not
hold, F2 falls back to a runtime flag and two guarantees drop from `typestate` to `checked` —
a retreat worth discovering in **S0**, not in S4.

The second ordering rule is `RUST-DESIGN-REVIEW.md:147`'s: centralise parsing, introduce the types,
convert boundaries, propagate, add the outcome enums, isolate the pure transforms, and add typestate
only once the transitions are clear. The one deliberate deviation is that S1's identity newtypes and
S3's batch shape land **before any backend**, because they appear in every signature and retrofitting
them is the expensive direction.

---

## The slices

| # | slice | size | depends on | implements | discharges | status |
|---|---|---|---|---|---|---|
| S0 | Transaction-signature spike | S | — | F2 | nothing durable; de-risks S4 | **retired.** The answer was yes on all three counts; the crate is deleted and its nine compile-fail cases are accounted for in ADR-0030 §2.5 — four were already duplicated against the shipped types, four were folded into `cyrup-pico`'s suite, and one was dropped with its reason. |
| S1 | Identity and value newtypes | S | — | F6 §A | `G-IDS-DISTINCT`, `G-SEQ`, `G-LIFETIME-HALF-OPEN` | landed |
| S2 | The pure document core | L | S1 | F4, F6 §C | `G-INV-6` (value half), `G-TRUSTED-IMMUTABLE`, `G-OWNERSHIP-BOUNDARY`, `G-CORRUPTION-NOT-ABSENCE`, `G-VERSION-PER-RECORD` | landed |
| S3 | Storage contract + memory backend + conformance harness | L | S1, S2 | F3, F6 §B | `G-INV-1` (shape), `G-ONE-CONTENT-COMMAND`, `G-RETIRE-ORDER-FREE`, `G-SINGLE-COMMITTER`, `G-CURSORS-BACKEND-OWNED`, `G-MEMORY-IS-REFERENCE` | landed |
| S4 | The session kernel: line, transaction, drafts, witnesses | L | S0, S2, S3 | F1, F2, F5 | `G-INV-2`, `G-INV-3`, `G-INV-4`, `G-INV-6`, `G-INV-7`, `G-INV-8`, `G-READ-BEFORE-FIRST-TABLE-WRITE`, `G-PRE-ADMISSION-ROLLBACK` | landed |
| S5 | Documents in the session: definitions, scope, migration, checkpoints | M | S4 | — | `G-SCOPE-DETERMINES-LIFETIME`, `G-TOKEN-AGREES-WITH-RECORD`, `G-ONLY-TYPED-ACQUISITION-CREATES`, `G-CHECKPOINT-ONCE`, `G-MIGRATION-ACCESS-DRIVEN`, `G-FIRST-WRITE-AFTER-MIGRATION-IS-BASE` | landed |
| S6 | Observation and publication | M | S4 | F1 (observer half), F5 | `G-INV-3`, `G-SYNC-OBSERVERS-CAPTURE-ONLY`, `G-DOC-SOURCE-NO-MUTABLE-OBJECT` | landed — **minus the `G-INV-2` / `G-INV-3`-emitter compile-fail case** the crate's own guarantee table cites by a name (`a_publication_cannot_be_forged`) that no file carries. See ADR-0030 §2.5. |
| S7 | The JSONL backend: store lock, marker protocol, recovery | L | S3 | F6 §D | `G-INV-1` (in the medium), `G-JSONL-MARKER-PROTOCOL`, `G-JSONL-DURABILITY-TIERS`, `G-JSONL-RECOVERY-AND-POISON`, `G-ONE-WRITER-PER-STORE` | landed |
| S8 | Fault injection at three points | M | S4, S7 | — | verifies `G-INV-8`, `G-REJECTED-NO-DURABLE-EFFECT` | landed |
| S9 | Forks and copy-source isolation | M | S5, S7 | — | `G-COPY-SOURCE-SNAPSHOT-ISOLATION`, `G-FORK-POINT-ONE-ENTRY`, `G-FORK-POLICY-PERSISTED` | landed |
| S10 | Kernel indexes and query paths | L | S3, S7 | — | `G-KERNEL-INDEXES-REQUIRED`, `G-COMMIT-VISIBLE-TO-LATER-READS` | landed — measured: p95 open **29.5 ms** and resident index **0.045 MB** at 601 commits / 613 table records, against triggers of 200 ms and 64 MB. **The trigger does not fire.** |
| S11 | `durable_rename` fix in `cyrup-session` | S | — | F6 §D (half) | pre-existing defect; precondition for S7's reclamation | landed — unix arm only; the Windows arm is ADR-0030 §14 item 1 and is still open. |
| S12 | The SQLite backend | L | S3, S10, **trigger** | — | `G-BACKEND-PARITY-BY-CONFORMANCE`, `G-SQLITE-ONE-TX-PER-COMMIT` | not started, correctly — its trigger has not fired (see S10). |

**Totals: twelve slices** — ten unconditional, one spike, one conditional on a measurement.

**Where this stands.** Eleven slices have landed and S12's trigger has not fired, so the plan is
complete as scoped. The workspace is green: `cargo fmt --all -- --check`,
`cargo clippy --workspace --all-targets -- -D warnings` (0 warnings over all 31 members) and
`cargo nextest run --workspace` (**13 026 tests, 13 026 passed, 10 skipped**, 189 s), of which 313
are the five pico crates'. 55 `trybuild` compile-fail cases back the `unrepresentable` and
`typestate` rows of ADR-0030 §2 — and **ADR-0030 §2.5 lists the five rows they do not reach**, which
is the outstanding work this plan does not yet have a slice for.

---

### S0 — Transaction-signature spike · **S** · depends on nothing

A throwaway crate, deleted or folded into `cyrup-pico` when it answers its question. It has, and it
was: see the closing note below.

Write ADR-0030 F2's signature and prove, against a real borrow checker, that:

- `F: for<'tx> AsyncFnOnce(Tx<'tx, Reading>) -> Result<(R, Tx<'tx, Writing>), E>` compiles and is
  callable with an ordinary async closure;
- `Tx<'tx, Reading>::writing(self) -> Tx<'tx, Writing>` threads through, and `conversation()` on
  `Tx<Writing>` is a *method-not-found* error rather than a lifetime error (the message matters:
  this is the error a developer will hit);
- `Draft<'d, T>` obtained from `&'d mut Tx<'tx, _>` cannot be assigned to an outer binding, cannot
  be returned, and cannot be `tokio::spawn`ed;
- a future that borrows the `Tx` cannot be spawned;
- a `DocHandle<'tx, T>` from one `commit` call cannot be passed to another.

**Deliverable:** four `trybuild` compile-fail cases (ADR-0030 §7's list, minus the `SessionMut` one
which needs S4), and a one-paragraph verdict on ergonomics. **If the primary form does not hold**,
record the fallback (`&'tx mut Tx<'tx, Reading>` plus an internal flag) and downgrade `G-INV-4` and
`G-READ-BEFORE-FIRST-TABLE-WRITE` from `typestate` to `checked` in ADR-0030 §2 before S4 starts.
This is ADR-0030 open question 4, and nothing else in the plan depends on its answer.

**Answered, and retired.** The primary form holds, is callable with a plain `async |tx| { .. }`, and
rejects all five programs with readable errors — so `G-INV-4` and `G-READ-BEFORE-FIRST-TABLE-WRITE`
stay `typestate` and the fallback is not needed. One sub-answer constrains F2: the error type in the
bound must be **concrete**, because an unconstrained `E` turns six of the negative cases into
`E0282 type annotations needed`; F2 already writes `CallbackError`, so this confirms the ADR. One
finding goes the other way: §10's `_not_send` marker on `Tx` is not what rejects a spawned
`Tx`-borrowing future — `'static` alone does that — and what the marker actually buys is a `!Send`
`commit` future, which is what F1's actor needs. `cyrup-pico`'s
`a_future_borrowing_the_transaction_cannot_be_spawned` and `the_commit_future_is_not_send` carry both
halves against the shipped types. The spike crate is deleted; ADR-0030 §2.5 records case by case where
each of its nine went.

---

### S1 — Identity and value newtypes · **S** · depends on nothing

`crates/cyrup-pico-store`: `IdKind` (sealed, five kinds), `Id<K>(NonZeroU64, PhantomData<fn() -> K>)`,
`Seq(NonZeroU64)`, `DefVersion(NonZeroU32)`, `DocumentPoint`, `Lifetime`, `PageLimit`, `StoreId`,
`RawId`. Private fields, no `From<u64>`, no `Default`, no round-tripping accessor.

Hand-written `Deserialize` for each — never derived — per ADR-0030 §10's serde table: require a u64,
reject a string, a float, a negative and zero, and build through the private constructor.

**Settle ADR-0030 open question 8 here**: the `cyrup_core::EntryId` name collision. Nothing else can
be written until the name is decided, and `cyrup-pico-store` must not depend on `cyrup-core`.

**Tests.** One parser test per rejected shape per type; `Lifetime::new` rejecting an inverted
interval while accepting the empty one; `Lifetime::contains` table-testing the `<=`/`<` asymmetry at
both boundaries. No compile-fail tests — the constructors are private and unit tests cover them.

**Why first:** these appear in every signature in both crates, and
`RUST-DESIGN-REVIEW.md:147` applies literally.

---

### S2 — The pure document core · **L** · depends on S1

`crates/cyrup-pico-doc`, with **no `tokio`, no storage, no `Session`, no async**. This is the
functional core and every function in it is unit-testable with literal values.

- `DocValue`, `DocMap`, `DocRoot`, `JsonNum = serde_json::Number`. **No variant with interior
  mutability** — the rule F4 rests on.
- `DocValue::parse` as a custom `serde::Serializer` whose `serialize_f64`/`serialize_f32` reject
  non-finite. `serde_json::to_value` must not be reachable from this crate's document path.
- `Path`/`Seg`, `at_mut` walking the mutation spine with `Arc::make_mut` (structural sharing as a
  refcount consequence).
- Chord's seven operation tuples as `Op`/`OpBatch`; `apply`, `apply_batches`.
- `StoredContent`, `ReplayPlan::parse` (the two corruption arms), `materialize` (pure and total).
- `StoredVersion` witness — no serde in either direction.
- `choose_representation` and `classify_version` as pure functions: the checkpoint predicate's inputs
  are exactly `(candidate, ops, deltas_since_base)` and need no storage read (`spec.md:1392-1396`).
- No-op normalisation: an empty prepared batch on an existing current-version document writes
  nothing; a non-empty batch deeply equal to its base is still a real change. `IndexMap`'s
  order-insensitive `PartialEq` plus `serde_json/preserve_order` (already on, `Cargo.toml:192`) give
  Chord's exact semantics free.

**Tests, all pure.** `DocValue::parse` with a **named negative case for `f64::NAN`** — this is the
regression F4 exists to prevent, and the test is the artefact that stops someone reintroducing
`to_value`. A depth-limit test. `ReplayPlan::parse` table-tested over literal record sets for
missing-base and version-in-tail. `materialize` round-trips. Structural sharing asserted by
`Arc::ptr_eq` on an untouched subtree before and after a change. One canary test that the authority
revision is unchanged after a draft write (the refcount-≥-2 invariant of the Change constructor).

---

### S3 — Storage contract, memory backend, conformance harness · **L** · depends on S1, S2

`crates/cyrup-pico-store`, continued.

- The keyed `Batch` (five `BTreeMap`s, private fields, built only by the assembler) and
  `DocumentCommand`/`Retire`/`DocumentBase`/`DocumentContent` — F3.
- `trait Storage` with `commit(&mut self, batch, cx) -> Result<Seq, CommitError>`,
  `mint<K>(&mut self, cx)`, and `&self` read paths returning owned or `Arc<immutable>` values.
- `CommitError::{Rejected(RejectedReason), Uncertain(..)}` with `RejectedReason` **closed**, no
  string arm, and deliberately no `From<std::io::Error>`.
- `StorageFailure::{Corrupt, Io, HistoryNotRetained}` — absence and corruption as different channels.
- Per-scan cursor types over `CursorBytes { store: StoreId, payload }`, with no `Deserialize`.
- `MemoryStore` as §11.1's **normative reference semantics** — and note it is *simpler* than pi's,
  because `memory.ts:92-100`'s recursive clone has no Rust equivalent (F4). Its remaining job is the
  cross-batch checks F3 does not lift: address occupancy and global id ownership against committed
  records, derived from the per-table indexes rather than a separate ownership table.
- The conformance harness skeleton behind `feature = "conformance"`, driving a `&mut dyn Storage`
  with **no Session**, so §10:4272-4277's trust split is testable.

**Tests.** The conformance suite's first cases: read-your-commit; sequence strict increase; mint
monotonicity; detachment (a committed value cannot be observed to change); the two cross-batch
rejections. **One canary each** for the states F3 made unspellable, on the module's privacy boundary
— not a matrix.

**Runs in parallel with S2** after S1 lands, with one coupling: `DocumentBase` takes `DocRoot` and
`DocumentContent::Delta` takes `StoredVersion`, both from S2.

---

### S4 — The session kernel · **L** · depends on S0, S2, S3

`crates/cyrup-pico`. The architectural commitment, and the slice S0 exists to de-risk.

- `Session(Arc<SessionShared>)` — clonable, reads and subscriptions only, **no commit method and no
  mutable document accessor**, which is how §3's sole-mutator rule becomes a type's method set.
- `SessionMut` — not `Clone`, never in an `Arc`, owning `Box<dyn Storage>`, with a one-way
  `admitting` flag sealed first by `close()`.
- `Committer(mpsc::Sender<Job>)` and the actor loop that is the only place `SessionMut` lives.
- `CommitOutcome` with four named variants and the handle inside the three non-fatal ones;
  `UncertainKind` with **both** poison paths; `CommitReply` for ticket holders.
- `Tx<'tx, Reading>` / `Tx<'tx, Writing>`, invariant `'tx`, `!Send`, `!Clone`, no Session handle and
  no effect capability; `Draft<'d, T>` borrowing the open change; `DocHandle<'tx, T>` memoised by
  logical address before its first await.
- `Durable` and `Publication` witnesses, crate-private, `_seal: ()`, no serde; `adopt` consuming
  both; `publish` consuming the publication.
- The debug-build line-hold budget, labelled a runtime check, because `G-INV-4` is `guarded`.

**Tests.** The four `trybuild` cases from ADR-0030 §7, now including *a `SessionMut` cannot be
obtained from `CommitOutcome::Uncertain`*. Behaviour tests for: `NothingToCommit` allocating no
sequence; `RolledBack` leaving the Session usable; both `Uncertain` paths killing the loop and
closing every ticket; `adopt` being allocation-free (assert by counting); a nested commit through a
`Committer` from inside a callback being impossible to write at all (the canary is that the type has
no such method). **Delete nothing yet** — the runtime checks ADR-0030 §7 lists stay.

---

### S5 — Documents in the session · **M** · depends on S4

Definitions and tokens; `DocumentRecord` and its index; `DocAddress` (kind + scope + optional key,
with a missing key meaning *the singleton*, not *every family member*); `Scope` and
`ConversationSemantics` enums making `fork: asOf` without `history: rewindable` unspellable in the
token; the token-versus-persisted-record agreement check (a runtime check, **correctly** so —
`spec.md:1063-1065` makes the record the authority precisely so unavailable extension code cannot
make data disappear); get-or-create as the *only* creating path; family-seed-consumed-only-when-absent;
the checkpoint predicate evaluated exactly once with a count excluding the change being evaluated;
the four-way migration ladder (`equal` / `older` / `newer` / `older-with-no-callback`) as an enum, and
the required-base flag that survives coalescing; retirement persisting final content first; and
reclamation gated on the persisted history policy, with a numeric lookup of a current-only
incarnation **rejecting** rather than guessing.

**Tests.** Parser tests for `DocAddress` singleton-versus-keyless. Behaviour tests for the migration
ladder (four arms), the first-write-after-migration base even when the migrated value is deeply
equal, and unaccessed documents with unavailable definitions surviving a reopen byte-identically.

---

### S6 — Observation and publication · **M** · depends on S4

`CommitObserver`/`CloseObserver` as `Fn(&Publication<'_>) -> ()` traits with no Session handle;
`Disposer` idempotent by construction (a `Weak` slot plus `Drop`); `DocState<T>` and the `Attachment`
whose snapshot and buffering registration are **one value from one constructor**, activated by a
consuming `activate(self)`; `DocWatch<T>` with a consuming `start(self)`; `Revision<T>` with
`raw() -> &Arc<DocRoot>` and a fallible `typed()`; `FrameContext` carrying values and **having no
token field**, so inheriting the producer's cancellation is unrepresentable.

**Carries ADR-0030 open question 6** — cyrup has no value-carrying `Context` — so the
`FrameContext` shape may change. Nothing else depends on it.

**Tests.** Publication across observers is all-or-nothing, **in a debug build**, because
`[profile.release]`'s `panic = "abort"` (`Cargo.toml:447`) removes the partial-advance hazard
degenerately. Buffered-watch ordering (exact committed frames, in commit order, on activation). One
canary that a published revision has no `&mut` path. A test that `Revision::typed()` failing is one
broken document and not a Session failure.

---

### S7 — The JSONL backend · **L** · depends on S3 (and S11 for the rename)

`crates/cyrup-pico-store-jsonl`.

- `StoreLock` (RAII, `flock(LOCK_EX|LOCK_NB)` / `LockFileEx`, via `fs4`) as a **required argument**
  to `open_for_write`; `open_read_only` taking no lock and having no `commit`. Plus a durable store
  identity record — `StoreId`, format version, holder pid — so a stale or ignored lock is detectable.
- `Durability::{ProcessCrash, PowerLoss}` as a named enum, wired to `cyrup-session`'s existing
  `SESSION_SYNCER` (`store.rs:65-182`) so the strong tier costs ~1.5 µs on the caller thread rather
  than ~214 µs.
- §11.3's protocol: append complete records to every affected sidecar; for `PowerLoss`, flush each
  affected sidecar; then append one complete main marker; publish only after the marker succeeds.
  No standalone-sidecar fast path.
- **The marker carries sidecar byte offsets** — sound only because the lock makes this process the
  only appender. A `CYRUP-DELTA`: same §10 guarantee (no open-time all-document scan), cheaper
  mechanism. Recovery must validate every offset against actual sidecar length and treat past-EOF as
  corruption. **This is ADR-0030 open question 3**; if the validation is too expensive, fall back to
  pi's marker shape and lose only the open-time saving.
- Recovery: torn final lines removed; unconfirmed sidecar tails ignored and removed; only confirmed
  records applied; **missing required confirmed data fails the open** rather than returning less;
  markers strictly increasing.
- A backend poison flag — `CommitError::Uncertain` its only producer — readable from the `&self` read
  paths, which is why ADR-0030 §5 rejects typestate here.
- Reclamation only after the authorising base or retirement committed, with the main log flushed once
  first; if that flush fails, committed state stays published and reclamation is deferred.
- The fd-invalidation discipline, lifted from `cyrup-session/src/store.rs`'s `rewrite`, which already
  has it and already explains why.

**Tests.** The reopen suite: exactly the set of commits whose markers survived. A crash injected
**between mint and commit**, asserting no id is reissued. Both fsync tiers, with the asymmetry
asserted explicitly: a lost tail commit is recoverable, a marker without its data is not. A second
process's `open_for_write` refused with the holder named; a second process's `open_read_only`
succeeding and not seeing an unconfirmed tail.

---

### S8 — Fault injection · **M** · depends on S4, S7

Injected failures at **three** distinguishable points, not two:

1. **before admission** — expect `RolledBack`, Session usable, nothing published;
2. **admitted then unknown** — expect `Uncertain { kind: CommitStateUnknown }`, no publication, every
   ticket dead, store reopenable;
3. **committed then adoption failed** — expect `Uncertain { kind: AdoptionFailedAfterCommit { seq } }`,
   and assert the store *does* contain the batch on reopen. **This is the point a port will skip**,
   and it is the one pi poisons on that nobody expects.

Plus the dishonest-backend cases: a `Rejected` returned after a partial durable write must be caught
by the suite, not inferred — because no type catches it.

---

### S9 — Forks and copy-source isolation · **M** · depends on S5, S7

A fork points at one visible entry and one commit; `ForkPolicy::{AsOf, Current, Initial}` read from
the persisted record, never from the token; a transaction that creates a fork and also writes one of
the parent's `current`-forking documents is rejected **before admission**; a copy reads committed
pre-batch source state independent of command order and persists one independent complete base at the
source's stored version; task documents are never copied.

**Tests.** The order-independence case explicitly: the same program with the copy command assembled
before and after the parent's write must produce the same child — which under F3's keyed batch is not
even expressible as two different batches, so this becomes one canary plus a behaviour test that
later source changes, retirement and reopen cannot affect the child.

---

### S10 — Kernel indexes and query paths · **L** · depends on S3, S7

The named access paths, with the ancestry cap applied **inside** the backend so a caller cannot
answer them by scanning: `find_latest_head_marker`, `scan_entries` (inclusive id ranges, newest-first
paging, every conversation ancestry cap applied), conjunctive indexed conversation-owner filters, task
queries by five fields, exact document address resolution at current or a historical sequence, and
document materialisation as newest base plus ordered delta tail.

**Tests.** A query-plan suite asserting each path is answered from an index rather than a scan, and a
benchmark suite producing the two numbers **ADR-0030 open question 2 needs**: open time and resident
index size at a realistic commit count. This slice is what makes the S12 trigger measurable rather
than rhetorical.

---

### S11 — `durable_rename` in `cyrup-session` · **S** · depends on nothing

`crates/cyrup-session/src/store.rs:322-324` does `f.sync_data()?` then
`std::fs::rename(&tmp, &self.path)?` with **no parent-directory fsync**, so the rename is not durable
on unix. That is a defect in the tree today, independent of Pico5, and it is a precondition for S7's
reclamation path.

Add a `durable_rename(tmp, dst)` helper — fsync the file, rename, **fsync the parent directory** —
and use it in `rewrite`. **Carries ADR-0030 open question 1**: `std::fs::rename` maps to `MoveFileEx`
without `MOVEFILE_WRITE_THROUGH` and Windows has no directory fsync, so the Windows arm needs a
decision (`windows-sys` and a direct `MoveFileEx` call, NTFS journalling accepted as sufficient, or a
stated weaker envelope). `docs/adr/ADR-0007-windows-scope.md` puts Windows in scope, so this is an
answer, not a `cfg`-gated TODO.

**Fully independent of every other slice.** It can land first, last, or beside anything.

---

### S12 — The SQLite backend · **L** · depends on S3, S10, **and the trigger**

**Do not write this slice until the trigger fires.** From ADR-0030 §9:

> Write `cyrup-pico-store-sqlite` when a p95 real cyrup session's store exceeds **either** 200 ms to
> open **or** 64 MB resident index.

S10's benchmark suite produces those numbers. The threshold itself is a placeholder with no
measurement behind it (ADR-0030 open question 2) and should be re-stated once S10 has measured
reality.

When it lands: one SQL transaction per Session commit, covering record tables, document records and
the indexed bases and deltas keyed by `(document, seq)`; the commit sequence allocated from a
metadata row **inside** that transaction; live task transitions replacing one row and terminal tasks
remaining small queryable records; `rusqlite` with `bundled`, and the crate **outside
`default-members`** so a plain `cargo build` never compiles the C amalgamation and the Windows build
stays green. Schema shape, WAL checkpoint cadence and `synchronous` default are backend choices
validated by conformance, not contract (`spec.md:4393-4396`).

Its whole job is to pass S3's conformance suite unchanged. That is the point of landing the suite in
S3.

---

## Parallelism

```
S0  ──────────────────────────────┐
S1  ──┬── S2 ──┬───────────────── S4 ──┬── S5 ──┬── S9
      └── S3 ──┘                       ├── S6   │
                 └── S7 ──┬── S8 ──────┘        │
                          └── S10 ──── (trigger) ── S12
S11 ─── (independent, any time)
```

- **S0 and S1 start together**, and S0 runs beside S1 and S2 throughout — it blocks only S4.
- **S2 and S3 run in parallel** once S1 lands, with one coupling (S3's `DocumentBase` takes S2's
  `DocRoot`, and `DocumentContent::Delta` takes S2's `StoredVersion`). Two people, two crates, no
  shared files.
- **S7 can start as soon as S3's trait is stable**, in parallel with S4 — it is a backend and does
  not need the Session. This is the second-largest parallelism win and the reason S3 ships the
  conformance harness rather than deferring it.
- **S5 and S6 are independent of each other** once S4 lands.
- **S11 is fully independent** and should be taken by whoever is idle first; it is also the smallest
  piece of real user-facing value in the plan.
- **S8, S9, S10 are mutually independent** given their predecessors.

**Critical path:** S1 → S2 → S4 → S5 → S9, with S0 gating S4 and S7 joining before S8.

---

## What is not in this plan, and why

- **§5–§9, the harness** — out of scope per ADR-0029. The kernel is shaped so they can be added on
  top without reshaping it, and ADR-0030 §2.4 records how they would be represented (enums, not
  typestate) so that nobody re-derives it. If the durable task machine is ever wanted, the move is to
  put `cyrup-agent`'s existing loop *on* the kernel, not to grow a parallel one beside it.
- **Anything on §13's non-goals list** — a whole-Session DOM, an optimistic progress channel, a
  kernel event journal, session-scoped rewindable documents, an automatic checkpoint heuristic, CRDT
  merge, SQL translation of Chord operations, JSONL compaction, automatic corruption repair, a
  Pico-prototype compatibility layer, forced termination of non-cooperative code. Designing these in
  is over-building, not ambition.
- **Migration of cyrup's existing file sessions** — ADR-0030 open question 9, a product decision, not
  scheduled.
