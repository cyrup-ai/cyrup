# ADR-0030 — Pico5 in cyrup: enums for the durable machine, typestate for the transaction, and ownership instead of half the spec

**Status** accepted (decided by default under the parity rule — overridable)
**Date** 2026-10-02
**Decides** the Rust type-design question for cyrup's Pico5 durability kernel, whose scope `docs/adr/ADR-0029-durable-pico5-scope.md` sets. Applies `docs/RUST-DESIGN-REVIEW.md` to `packages/durable` @ pi `v1.0.0` and its `docs/spec.md`.
**Blocks released** every slice in `docs/PICO5-PLAN.md`; the crate layout for `cyrup-pico-doc`, `cyrup-pico` and `cyrup-pico-store-jsonl`; the storage-engine trigger; and the `CYRUP-DELTA` notes that record where cyrup deletes pi machinery rather than porting it.

---

## Context

This is an **opportunity review written before its subject exists**, the second in this directory
after `docs/adr/ADR-0028-cyrup-acp-type-design.md`, and it follows `docs/RUST-DESIGN-REVIEW.md`'s
required output structure in order. Its subject is reconstructed from pi `v1.0.0` — read only with
`git -C tmp/pi show v1.0.0:<path>` — and from the cyrup types the kernel will sit on. No code was
modified to produce it.

The upstream authority is `packages/durable/docs/spec.md`, 4 601 lines: §1's eight numbered
invariants (`:63-78`), §2's record set (`:80-288`), §3's documents (`:919-1520`), §4's transactions
and storage ownership (`:1521-1567`), §5's tasks (`:1568-2119`), §9's observation (`:3747-4187`),
§10's storage contract (`:4188-4369`), §11's three backends (`:4371-4448`), §12's thirty-four API
footguns (`:4450-4585`) and §13's eleven non-goals (`:4587-4601`). Where this document cites an
upstream implementation detail it names the file and line at v1.0.0.

§12 opens with the sentence this whole review turns on:

> These are contracts, not invitations to add defensive machinery.

Nearly every one of those thirty-four contracts is a hazard TypeScript can only reject at runtime.
The question asked here is never *what does this `.ts` file do*; it is **what must be true, and what
is the cheapest Rust mechanism that makes violating it impossible to express.**

Read this as an opportunity review. It recommends the smallest change that captures a real
invariant, §5 is where it argues *against* applying a pattern, and every finding states the
guarantee gained **and** the guarantee not gained — because the difference is exactly where the
runtime tests still have to live.

---

## 1. Executive verdict

This subsystem needs, in order: **explicit domain enums for everything durable; newtypes with
parse-don't-validate at every boundary a number or a value enters; a functional core / imperative
shell split on the storage seam; and typestate in exactly three narrow, consuming places.**

**Typestate is justified, and this is the inverse of ADR-0028's verdict — on one clause.** ADR-0028
rejected typestate because *"the ACP SDK registers every handler for the life of the connection —
there is no ownership path along which a compiler could withhold a method"* (ADR-0028:42). In §4
there is exactly such a path, and there are three of them:

1. the transaction is **handed to a closure and taken back**, so `Tx<Reading> → Tx<Writing>` can
   withhold `read()` after the first table write (§4's own rule, `spec.md:1553-1557`);
2. a draft **borrows** the transaction for the closure's lifetime, so it cannot escape;
3. the mutation handle is **moved into the commit and returned only on a non-fatal outcome**, so an
   uncertain storage failure leaves no usable Session behind.

That clause is the only one that separates the two subjects. Every rejection test at
`RUST-DESIGN-REVIEW.md:55-61` passes for all three: the state is not persisted, not resumed, not
stored heterogeneously, not selected by external events, and the compiler-visible lifecycle **ends
when the closure returns** rather than continuing into a distributed one.

**And typestate is wrong for the largest thing in the subject, which must be said just as plainly.**
A task is, in the spec's own words, *"a durable state machine attached to one conversation"*
(`spec.md:51`). Its five states (`spec.md:1580-1594`) are persisted as a JSON `checkpoint` inside the
task record, reconstructed from storage, held heterogeneously in one `TaskId`-keyed table, and
selected by a scheduler reacting to external events; one phase-handler invocation is the only part
the compiler sees, and the real lifecycle spans process restarts. **That is an enum, however stateful
it looks**, and it is the textbook case of `RUST-DESIGN-REVIEW.md:61`'s last rule. So are
`TaskOutcome`'s five variants, `TaskOwnership`'s two, `ConversationOwnership`'s two, the three
built-in task kinds, `DocumentPoint`, and a document's lifetime interval. A design that typestates
the task machine is rejected here in advance (§5).

**The highest-value opportunity is one owned, `Arc`-shared immutable document value
(F4).** Chord's own README opens with the sentence that makes the case:

> Immutability is an ownership contract. Nothing is frozen or defensively copied, so an illegal
> mutation is not detected. It silently corrupts state.

`Arc<DocValue>` over a type with no interior mutability turns that contract into a type. It
discharges four guarantees at once, makes eight of Chord's ten rejected placement shapes
unrepresentable with no check at all, and **deletes** rather than ports: Chord's whole mutation-rights
table, the alias-free-root rule, §4's entire storage-ownership section (`spec.md:1560-1566`), and
`memory.ts:92-100`'s recursive `clone` — whose own comment at `:216` admits it exists *"to match the
ownership boundary"* that SQLite and JSONL get free from encode/decode. That is the single largest
body of runtime work in the specification that Rust makes free.

The second is **the keyed commit batch (F3)**, which stops five illegal states from being
expressible and deletes the hand-written validation all three pi backends carry for them
(`memory.ts:689-695, 716-759` and SQLite's `checkDocumentActions()`), including the ordering rule
§10:4363 has to impose on a representation that has an order.

**The major tradeoff** is that pi's draft is a JavaScript `Proxy` over ordinary property assignment
(`change.state.output += "done\n"`), and Rust has no equivalent for field assignment. Draft mutation
becomes explicit path-addressed method calls. That is a mechanism difference at full feature parity
— a legitimate `CYRUP-DELTA` — and it is the one place where cyrup's surface is **less** ergonomic
than pi's rather than more. The escape (type the document as a host struct and diff old against new)
is rejected in §5: Chord's operations are *exact*, not a diff, and a differ either collapses every
write to a whole-value replacement — destroying the base/delta/checkpoint economy the whole storage
design is built on — or is strictly more machinery than recording the mutation that happened.

**A second tradeoff, stated because it is the one most likely to be overclaimed:** §1 invariant 4
(*no external effect inside the mutation transaction*) is **guarded, not unrepresentable**. Handing
the closure no effect capability and no Session handle removes every effect reachable *through* the
transaction — the entire intended path, and the whole of the nested-commit deadlock — but a closure
that captures an `Arc<ModelClient>` from its environment still compiles and still holds the line.
There is no Rust construct that says *"this async block may only await futures derived from this
parameter."* The honest additional measure is a debug-build line-hold budget, and it is labelled a
runtime check.

**Missing domain information**, all carried as open questions in §14: cyrup has no value-carrying
`Context` analogue (65 `CancellationToken` uses in the tree and nothing that carries values), so the
frame-context design is provisional; the UTF-16 code-unit basis of Chord's string-trim operation is a
persisted-format decision nobody has made; `cyrup_core::EntryId` already exists as a string id while
Pico5's `EntryId` is a number in one global namespace, which is a naming collision that must be
settled before a signature is written; whether cyrup's existing file sessions migrate into a Pico5
store at all; Windows durable rename; and the storage-engine trigger needs a measurement nobody has
taken.

---

## 2. Invariant and state map

One row per Pico5 guarantee. **Enforcement** is the classification `RUST-DESIGN-REVIEW.md` asks for,
with the four values this subject needs: *unrepresentable* (a compile error), *typestate* (a state
transition), *guarded* (an RAII/ownership/private-constructor guard), *checked* (a runtime check,
with the reason it cannot be lifted).

### 2.1 The eight required invariants (`spec.md:63-78`)

| Location | Domain fact or state | Current encoding (pi) | Failure mode | Best representation (cyrup) | Enforcement |
|---|---|---|---|---|---|
| §1.1, `session.ts` `#runCommit` | one commit is atomic across all record and document writes | one `StorageWrite[]`, one `Storage.commit()`, atomicity re-validated per backend; cross-backend equality asserted only by the conformance suite (`spec.md:4369`) | a torn commit shows an answer whose task says it never ran; no later repair can tell which half was true | keyed `Batch` taken **by value** (F3); one SQL transaction / one marker append per backend | unrepresentable (Session cannot split a change) + **checked** (atomicity in the medium — conformance + fault injection) |
| §1.2 | no document update is visible before its commit succeeded | `#publish()` has exactly one call site, placed after `await commit()` and after `tx.adopt(seq)`. Nothing prevents a second | an observer renders state a crash erases — the user watches the agent do something that never happened | `Durable → Publication` witness chain, private constructors (F5) | **unrepresentable** |
| §1.3 | all visible progress is durable; there is no volatile path | architectural absence plus a declared non-goal (`spec.md:4591`) | a client cannot treat what it saw as the session's state, so resume needs a reconciliation protocol that does not exist | the same witness: a second emitter has no `Publication` and cannot construct one (F5) | **unrepresentable** |
| §1.4 | no external effect runs inside the mutation transaction | convention and documentation only — the weakest-enforced of the eight. A nested `session.commit()` is not even diagnosed: it queues on `#tail` and deadlocks permanently | one 60-second model call freezes every commit in the process; a human-in-the-loop prompt deadlocks the session | `Tx` holds no effect capability and no Session handle; effects are the callback's return value, run after the await (F2) | **unrepresentable** for the reachable path and for nested commit; **guarded** for a captured handle; + **checked** (debug line-hold budget) |
| §1.5 | entries and ids are immutable and never reused after a committed write | a phantom brand erased at runtime, applied by two bare casts (`ids.ts:4-6, 9-11`, eleven lines total), plus per-backend collision checks (`memory.ts:689-695`) | a reused id re-points history at the wrong record: a fork rewinds to another conversation's entry | kind-tagged `Id<K>(NonZeroU64)`, private field, no `From<u64>`, no `Default`; minting only on the storage handle (F6 §A) | **guarded** at allocation; **unrepresentable** for kind confusion in-process; **checked** for non-reuse across reopen |
| §1.6 | drafts are fully revoked when the callback settles; assigned values are copied and strict JSON | a JS `Proxy` revocation **plus** a `#sealed` flag **plus** `#assertOpen()` on every `Tx` method (`transaction.ts:663, 673, 898-900`) **plus** a validating copy walk | a draft that still worked would accept writes never persisted and never published; the next commit silently overwrites | a draft **borrows** `Tx` for the closure's lifetime; `DocValue::parse` at the assignment (F2, F4) | **unrepresentable** — all four mechanisms deleted |
| §1.7 | the mutation line is held through storage settlement and adoption; line observers capture only | a single promise tail (`#enqueue`) plus a documented prohibition. A throwing listener skips every later listener **and** makes `commit()` reject although the commit is durable and adopted | the caller is told a durable commit failed; some view mounts advanced, others not | one non-`Clone` `SessionMut` owning `Box<dyn Storage>`, inside an actor; a clonable `Committer` ticket; observers are `Fn(&Publication) -> ()` with no Session handle (F1) | **unrepresentable** (one committer; cannot fail; cannot re-enter; cannot await); **checked** (continuity of the hold; panics) |
| §1.8 | an uncertain storage failure is fatal; pre-admission failures roll back normally | a `#poison` flag plus one `instanceof StorageRejected` test. The poisoned object stays in the caller's hands, so the mistake is a `catch` away — and §12:4581 has to *ask* people not to make it | continuing prepares the next change against a baseline that may already disagree with the disk: silent unbounded divergence instead of one loud failure | `CommitOutcome` with four named variants; the handle is **inside** the non-fatal variants and absent from `Uncertain` (F1) | **typestate** (consuming) at the kernel; **checked** at the shared `Committer` boundary, stated as such |

### 2.2 Records, identity and storage (§2, §4, §10, §11)

| Location | Domain fact or state | Current encoding (pi) | Failure mode | Best representation (cyrup) | Enforcement |
|---|---|---|---|---|---|
| `spec.md:242-245`, `ids.ts` | an entity id and a commit sequence are different kinds of thing | both are plain `number`; brands are compile-time only and cast from anywhere in the package | `snapshotAsOf` answers a different question and shows the user a confident, wrong past | distinct `Id<K>` and `Seq` newtypes, genuinely distinct at runtime | **unrepresentable** |
| `spec.md:4227` | a document read point is "a sequence" or "current" | `DocumentPoint = Seq \| "current"` | — | two-variant enum: inspected dynamically against stored values | **enum, by the decision rule** |
| `spec.md:1112-1114` | incarnation membership is the half-open interval `createdAt <= at < retiredAt` | two `Seq` fields and the interval test written at each read site (two sites) | retire-then-create at one address overlaps or gaps at the boundary sequence | `Lifetime` newtype, private fields, `contains()` the only accessor; empty lifetime legal, inverted rejected | **guarded** (one implementation of `<=`/`<`) |
| `spec.md:99-100`, `memory.ts:253`, `jsonl/storage.ts:543-546` | commit sequences strictly increase across the store's life, including reopen; gaps permitted | runtime checks at the write and recovery boundaries | a non-increasing sequence makes lifetime intervals and delta ordering meaningless | `Seq` newtype + the backend's check at both boundaries | **checked** — crosses a process boundary and cannot be lifted |
| `spec.md:4282`, `memory.ts:410-413` | ids come from one durable monotone global namespace | `mintId()` is a storage method; exhaustion checked with `Number.isSafeInteger` | an in-memory fast path or a stale cached high-water mark reissues ids | `mint<K>(&mut self)` on the storage handle only — no free function, no `Default` allocator, no public `Id` constructor; fallible, not wrapping | **guarded**; **checked** for high-water-mark durability (reopen suite) |
| `spec.md:4274-4275`, `memory.ts:689-695` | one number is owned by one record of one type | per-backend checks against committed state | id 41 is written as two record types, or re-points an existing record | per-table `BTreeMap<Id<K>, _>` kills the in-table and in-process cross-table cases; ownership against **already-committed** records stays a backend check | **unrepresentable** in-process; **checked** cross-batch |
| `spec.md:4362-4367`, `memory.ts:716-759` | one batch holds at most one content command per incarnation, plus an optional retirement; a delta needs a base; a version transition must be a base | a flat `readonly StorageWrite[]` that can express all five illegal states, re-validated by hand in all three backends | two content commands have no defined meaning; a delta across a version boundary makes the document permanently unreadable | keyed `Batch` + `DocumentCommand` carrying content *and* an optional retirement as one value (F3) | **unrepresentable** for four of five; **checked** for "address already has a current incarnation" (cross-batch) |
| `spec.md:4363-4364` | storage applies content before retirement, independent of write-array order | a prose rule each backend implements by hand after normalising the array | a retire listed before its content discards a terminal task's final state — exactly the document the user cares about most — and the bug reproduces on one backend only | no array: order is not expressible (F3). Order **within** one incarnation's delta tail stays a sequence (`spec.md:4350`) | **unrepresentable** |
| `spec.md:4307-4311`, `errors.ts:12-18` | "rejected, nothing durable happened" is distinguishable from "uncertain" | an exception subclass each backend chooses by hand, and one `instanceof` test | classified wrong one way, a bad fork source costs the user the whole live session; wrong the other way, the Session runs on a baseline that already disagrees with the disk | `CommitError::{Rejected(RejectedReason), Uncertain(..)}` where `RejectedReason` is a **closed** enum with no string arm and no `From<io::Error>` (F1) | **unrepresentable** to omit the choice or to classify I/O as recoverable; **checked** for whether the claim is true |
| `spec.md:1560-1566`, `memory.ts:92-100, 216`, `spec.md:4375-4378, 4402-4405` | storage and the Session never alias caller memory | explicit deep copying at every boundary, which §11.1 says outright exists to *simulate* the boundary SQLite and JSONL get free | a caller mutates something it already committed, or a backend's index shares a container with a reader's value: committed history changes under a user who is reading it, with no write and no event | ownership. `commit(batch)` by value; reads return owned or `Arc<immutable>`; no `#[serde(borrow)]`, no `Cow` in persisted records (F4) | **unrepresentable** — `CYRUP-DELTA`: same guarantee, mechanism deleted |
| `spec.md:4528-4530` | every published revision is immutable for all time, with structural sharing | a convention; *"runtime freezing is not provided"* | the quietest corruption available: a consumer sorts `value.tools` in place and every observer's retained revision is reordered | `Arc<DocValue>` over a type with **no interior mutability** (F4) | **unrepresentable**, conditional on the no-interior-mutability rule |
| `spec.md:4320-4321, 4196` | a cursor round-trips into the same scan on the same storage, and nowhere else | `type Cursor = Readonly<Record<string, JsonValue>>` — one structural bag for every scan and every backend | the scan resumes from a position meaning something else; a history page has a hole, with no error and no log line | one opaque cursor type per scan, no `Deserialize`, with a `StoreId` stamp inside (F6 §B) | **unrepresentable** cross-scan; **checked** cross-store |
| `spec.md:4338-4339` | `limit` is a maximum page size, never a guarantee | `number` — admits 0, negative, absurd | — | `PageLimit(NonZeroU32)` parsed with a `MAX`; `Page { items, next: Option<C> }` | **unrepresentable** for the value; doc contract for "maximum" |
| `spec.md:4352-4360` | a legitimately absent record is never reported as damaged data, or vice versa | the split is carried entirely by which call path throws versus returns `undefined` | reporting corruption as absence loses a user's work quietly: the session opens, the document is empty, the agent proceeds from a blank slate | `Result<Option<T>, StorageFailure>` — `Ok(None)` is absence, `Err(Corrupt(..))` fails the open; `ReplayPlan::parse` is where the two corruption rules live (F6 §C) | **unrepresentable** to confuse the channels |
| `spec.md:4321-4323` | the Session is the sole committer, so backends add no second commit mutex | nothing. An ordinary method on a shared interface; the property is documented of the caller and unverifiable by the callee | a second concurrent committer breaks sequence allocation and batch normalisation in all three backends | `commit(&mut self, ..)` with the handle owned exclusively by one non-`Clone` `SessionMut` (F1, F3) | **unrepresentable** in-process; **guarded** cross-process by `StoreLock` (F6 §D); **checked** on network filesystems |
| `spec.md:4325-4351` | storage owns the indexed access paths; an application registry is not a substitute, and there is no open-time all-document scan | named methods plus a query-plan test suite | if they degrade to scans, cost grows with total history: a long session gets slower until it is unusable — the agent dies of old age | keep them named methods with the ancestry cap applied inside; **this is the requirement that prices a storage engine** | **guarded** (cannot be answered by scanning in the caller); **checked** (whether a backend uses an index) |
| `spec.md:4407-4431` | JSONL atomicity is sidecars-then-one-marker; the fsync envelope is an explicit choice | implementation ordering plus a `fsync?: boolean` defaulting to **false** (`jsonl/storage.ts:77-78`) | a marker surviving without its data is corruption; losing a tail commit is merely recoverable — the asymmetry is the subtle part | `Durability::{ProcessCrash, PowerLoss}` as a named enum, not a bare bool; marker carries sidecar offsets (F6 §D) | **checked** — and note cyrup's existing default is already **stronger** than pi's |

### 2.3 Documents (§3, §9) — the mechanism behind invariants 6 and 7

| Location | Domain fact or state | Current encoding (pi) | Failure mode | Best representation (cyrup) | Enforcement |
|---|---|---|---|---|---|
| `spec.md:1126`, `:3822` | there is exactly one path by which a document changes: a draft inside a mutation transaction | *"There is no mutable `session.document()` API"* — API shape and convention | a second writer collapses ordering, atomicity and base/delta reconciliation, and would need the CRDT machinery §13:4596 refuses | `Session` has no commit and no mutable document accessor; `Draft` is reachable only from `Tx` | **unrepresentable** (structural) |
| `spec.md:72, 1356-1369` | only strict JSON enters a document, rejected at the offending assignment before the draft changes | a runtime copy walk rejecting `undefined`, non-finite numbers, functions, symbols, bigints, accessors, symbol keys, sparse arrays, foreign prototypes, cycles | a `NaN` that reaches storage makes the session unopenable, detected one restart later — total loss | Rust disposes of eight of the ten shapes with no check. The one that survives is real: `DocValue::parse` is a custom `Serializer` rejecting non-finite floats (F4) | **unrepresentable** for eight; **guarded** for non-finite floats at one boundary |
| `spec.md:1294-1303`, `:3947` | unchanged subtrees are structurally shared between successive revisions | implementation detail of `applyImmutable` | a 2 MB live document copies 2 MB per throttled commit | `Arc::make_mut` down the mutation spine: sharing is a refcount consequence, not code to port (F4) | **unrepresentable** to alias mutably; sharing is free |
| `spec.md:1305-1310, 1354-1356` | adoption is a synchronous pointer swap — no diff, no apply, no allocation, no callback | specified behaviour, enforced by implementation discipline | adoption runs after storage committed, where failure is unrecoverable: anything that can fail there turns a committed write into a poisoned Session | the candidate **is** the value, built incrementally as ops were recorded, so `prepare()` has nothing left to compute; `adopt` consumes `Tx` and `Durable` (F4, F5) | **guarded**; **checked** (an allocation-counting test) |
| `spec.md:1357-1361` | an empty prepared batch on an existing current-version document writes and publishes nothing; a non-empty batch equal to its base is still a real change | Chord normalises; the Session decides from batch emptiness plus required-base flags | a throttled live document would amplify every tick into a publication | `IndexMap`'s order-insensitive `PartialEq` matches Chord's *"equality ignores key order"* for free, and `serde_json/preserve_order` is already on workspace-wide (`Cargo.toml:192`) | **guarded** — free from an existing dependency |
| `spec.md:923-943, 1066-1071` | scope alone determines ownership and lifetime; only conversation documents carry history and fork policy | a TypeScript discriminated union for the token, **and a parallel union in the persisted record** that is re-read from storage where the union is not enforced | state private to a task leaks across a session, or state a user expects to persist disappears | `Scope` and `ConversationSemantics` enums; `asOf` legal only with `rewindable` is unspellable in the token | **unrepresentable** in the token; **checked** on the persisted record (F6 §C) |
| `spec.md:1063-1065, 1117-1120` | the persisted record, not the token, is the authority on scope and history | a runtime check at every typed access | a redefined token reinterprets a latest document as rewindable, so historical reads answer from legitimately reclaimed records | runtime check, deliberately: extension code reloads independently of its data, and unaccessed data must survive its code's absence | **checked** — correct as a check, and §12:4526 says why |
| `spec.md:1119-1120, 4366-4367` | a definition version belongs to each stored record; a delta can never cross a version boundary | `version` on every `DocumentContent`; a version change inside a tail is classified as corruption | a tail straddling a schema applies paths against the wrong shape: plausible-looking corrupt state, not an error | `ReplayPlan::parse` returns the two corruption arms; `materialize` is then total. A `StoredVersion` witness means a delta cannot claim a version it did not read (F6 §C) | **unrepresentable** downstream of the parser |
| `spec.md:1388-1399` | the checkpoint predicate is evaluated exactly once, after preparation, with a count excluding the change being evaluated, and performs no storage read | the Session calls it once at a fixed point and maintains `deltasSinceBase` itself | evaluating twice lets a stateful predicate observe an inconsistent count; a read puts I/O on the held line | a **pure function** over `(candidate, ops, deltas_since_base)` — a functional-core boundary, directly testable | **guarded** + pure core. §13:4594 forbids an automatic heuristic (§5) |
| `spec.md:1413-1416, 4354-4357` | a committed base permits reclamation for session/task/latest documents, never for rewindable history; a numeric lookup of a current-only incarnation **rejects** rather than guessing | history policy read from the persisted record gates reclamation | the only thing between "rewindable" being a promise and a best effort; answering a historical read from whatever records survive is silent | one method on the persisted record index, not a condition rewritten per call site | **guarded**; **checked** (reclamation ordering) |
| `spec.md:1441-1464` | migration is access-driven through the supplied token: equal uses, older migrates, newer rejects, older-with-no-callback rejects; the first write after a migration is a required base | a four-way comparison at each access; a required-base flag | a sweep at open would rewrite every extension's documents on every start, and one failing migration makes the session unopenable | a four-variant `VersionFit` enum + a `RequiredBase` flag that survives coalescing. A migration callback is **pure** and always receives a detached stored value | **enum** + **guarded**; `Arc<DocValue>` makes "a migration received a live revision" unrepresentable |
| `spec.md:1473-1506, 4313-4318` | a fork points at one entry and one commit; policy is persisted per incarnation; a copy reads committed pre-batch source state and the batch may not touch its source | per-backend validation against pre-batch state, reported as `StorageRejected` | without it, the child's value depends on whether the fork command was ordered before or after the parent's write — the same program producing two different children | `ForkPolicy` enum on the persisted record; the Session rejects the conflicting transaction pre-admission; copy independence by writing a complete base, not a reference | **checked** — cross-batch and cross-record by nature |
| `spec.md:1527-1533`, `session.ts` `#publish` | commit and close observers run synchronously on the line and may only capture immutable state | documented and unenforced; `#publish` has no try/catch | a throwing listener truncates the iteration *and* reports a durable, adopted commit as a failure | `Fn(&Publication, &Context) -> ()`: no `Result`, no `async`, no Session handle, borrowed publication (F1) | **unrepresentable** for all three prohibitions; **checked** for panics and blocking |
| `spec.md:4549-4552, 4523-4527` | the `pi.` prefix reserves built-in task names, document kinds and entry kinds | convention — §12's own words: *"nothing enforces it"* | an application entry kind `pi.system` is replayed as a system message, injecting the application's data into the model's system prompt | a `Kind` newtype parsed at registration that rejects the reserved namespace for third parties; built-ins through a crate-private path | **guarded**. Cannot help the persisted half: a kind change still needs explicit copy-and-retire, and §12:4526 keeps no registry by design |

### 2.4 The durable machine (§5) — recorded here precisely because it is **not** in scope for typestate

ADR-0029 puts §5–§9 out of build scope. These rows exist so that when the kernel is extended, the
representation is already decided and nobody reaches for typestate.

| Location | Domain fact or state | Current encoding (pi) | Failure mode | Best representation (cyrup) | Enforcement |
|---|---|---|---|---|---|
| `spec.md:1580-1594` | a task is a durable state machine with five states | a TypeScript discriminated union persisted as JSON in the task record, reconstructed at open, scheduled by external events | — | **`enum TaskState`**, exhaustively matched. `RUST-DESIGN-REVIEW.md:57` and `:61` both reject typestate: persisted, resumed, held in one `TaskId`-keyed table, selected externally, and the compiler-visible lifecycle (one phase invocation) ends long before the real one | **enum** — deliberately |
| `spec.md:1570-1578` | a task outcome is one of five named results | a five-variant union | — | `enum TaskOutcome` — named domain variants, per `RUST-DESIGN-REVIEW.md:84-88` | **enum** |
| `spec.md:1596, 1600-1604` | ownership is conversation-or-task, on both conversations and tasks | two-variant unions; the caller cannot omit the choice | an implicit or guessed owner silently changes which conversations an Escape cancels | two-variant enums, caller supplies a typed id, the Session derives the persisted pair. Attribution only — **not** an access-control capability, and nothing enforces that half | **enum**; the not-a-capability half is prose, as upstream |
| `spec.md:3168-3176` | the built-in task kinds are a closed set of **three** | three `TaskDefinition`s named `pi.generation`, `pi.tool`, `pi.compaction` | — | a closed enum of three, not five — area 17 says five; see ADR-0029 Measurement 1 | **enum** |
| `spec.md:1869-1884` | a run invocation may not commit after its durable abort mark appears; an abort handler cannot create owned work (§12:4553) | runtime rejection; the abort invocation's task is abort-marked | compensation logic in an abort handler creates work that is immediately marked, or never drains | the abort handler receives a **context type without the create-owned-work capability** — the one place in §5 where a capability-shaped type beats a check | **unrepresentable**, when §5 is built |

---
## 3. Findings

Five findings, which is the cap. A sixth entry follows it as a **grouped** finding of four small
supporting mechanisms; its §D exceeds the cap deliberately, on the ground
`RUST-DESIGN-REVIEW.md:126` allows — two `cyrup` processes in one directory silently corrupting a
store is a serious correctness risk, and it is a hazard pi does not have because pi's only consumer
is an unshipped frontend holding a `proper-lockfile`.

---

### [P1] F1 — One mutation handle, and a commit outcome that does not give it back

**Location.** Upstream: `session.ts`'s `#enqueue`/`#tail`, `#runCommit`, `#poison`,
`#assertHealthy`/`#assertUsable`, `#publish`, `#commitListeners`; `errors.ts:12-18`'s
`StorageRejected`. cyrup side: new.

**Current representation.** Serialisation is a promise tail; health is a `#poison` flag checked on
every entry point; the failure classification is one `instanceof StorageRejected` test over an
exception subclass each backend picks by hand; observers are untyped callbacks iterated with no
`try`/`catch`.

**The invariant.** One Session has one mutation line, held continuously from before the callback
through preparation, storage settlement, in-memory adoption and publication enqueue. A failure
before storage admission rolls back and the Session stays usable. A failure whose commit state is
unknown — **and, separately, a failure of adoption after storage already committed** — is fatal: the
Session must be reopened. Line observers run synchronously and may only capture immutable state.

**Concrete failure modes the current API permits.** Three, and the second is the one a port will hit.

*Misclassification.* The JSONL backend's sidecar append takes `ENOSPC` halfway through a 4 KB
record. A naive port wraps it in the backend's own error type; the kernel's `match` over a single
`StorageError` with a `Rejected` variant among a dozen classifies it recoverable; the Session
continues and prepares the next document delta against a base that is physically truncated on disk.
The user works for an hour and then the session will not reopen.

*The dropped second poison path.* pi poisons on **two** distinct paths, and only one is obvious. The
first is a non-`StorageRejected` failure from `storage.commit()`. The second is a throw from
`tx.adopt(seq)` **after storage has already committed** — `#runCommit`'s own comment reads *"Storage
already committed; a failed adoption leaves memory behind durable state."* A port that writes
`commit(self, ..) -> Result<(R, Seq, SessionMut), StorageError>` consumes `self` on commit failure
and gets invariant 8 half-right, while the adoption failure returns the handle and the Session
survives with memory known-stale behind durable state. The two need **different** recovery: the
first means reopen *and reconcile*; the second means storage is known-good and there is nothing to
reconcile.

*The `Arc<Mutex<Session>>` reflex.* A background compaction task and a foreground turn both want to
commit. With a shared handle and an internal mutex, the natural optimisation under review pressure
is a read path that takes the lock separately from the commit path — and then a snapshot read lands
between storage success and in-memory adoption, returning a durably stale value which it renders.

**Recommended pattern.** Explicit domain enums, with one consuming move. Not typestate on the
Session itself (§5 rejects `Session<Open|Closing|Closed>`).

```rust
/// Shared, cheap to clone. Reads and subscriptions only. No commit method and no
/// mutable document accessor exist on this type — §3's sole-mutator rule is this
/// type's method set.
#[derive(Clone)]
pub struct Session(Arc<SessionShared>);

/// The ONE mutation handle per open store. Not Clone, never in an Arc, never behind
/// a lock. It owns the storage handle, which is what makes `Storage::commit(&mut self)`
/// satisfiable.
pub struct SessionMut { shared: Arc<SessionShared>, storage: Box<dyn Storage>, admitting: bool }

#[must_use]
pub enum CommitOutcome<R> {
    /// The callback produced no writes: nothing reached storage, no sequence was
    /// allocated, nothing was published, nothing can poison. pi:
    /// `if (writes.length === 0) { tx.discard(); return result; }`.
    NothingToCommit { result: R, session: SessionMut },
    Committed      { result: R, seq: Seq, session: SessionMut },
    /// Callback error, draft validation, document preparation, checkpoint predicate,
    /// owner validation, assembly — or a batch storage rejected with rollback
    /// guaranteed. Nothing durable, nothing published, Session fully usable.
    RolledBack     { reason: RollbackReason, session: SessionMut },
    /// The Session is dead. There is NO handle in this variant.
    Uncertain(UncertainCommit),
}

pub struct UncertainCommit { pub kind: UncertainKind, pub source: StorageFailure }
pub enum UncertainKind {
    /// Storage did not say whether the batch committed. Reopen AND reconcile.
    CommitStateUnknown,
    /// Storage committed at `seq`; in-memory adoption failed. Durable state is
    /// known-good and known AHEAD of memory. Reopen; nothing to reconcile.
    AdoptionFailedAfterCommit { seq: Seq },
}

pub enum CommitError {
    /// MUST mean: rejected before any durable effect.
    Rejected(RejectedReason),
    /// Everything else. Commit state unknown by definition.
    Uncertain(Box<dyn std::error::Error + Send + Sync>),
}

/// CLOSED on purpose — NOT `#[non_exhaustive]`, no `Other(String)` arm, and
/// deliberately no `impl From<std::io::Error>`. Every variant is a deterministic
/// pre-durable check from §10:4307-4318. Adding one is a semver event reviewed
/// against the single question "is rollback guaranteed?".
pub enum RejectedReason {
    CopySourceMissing { source: DocumentId },
    CopySourceTouchedByBatch { source: DocumentId },
    CopySourceMismatch { source: DocumentId, field: SourceField },
    IdAlreadyOwned { id: RawId, by: IdKind },
    IdWrittenTwice { id: RawId },
    DocumentAddressOccupied { address: DocumentAddress },
    DeltaCrossesStoredVersion { document: DocumentId },
    SequenceNotIncreasing { proposed: Seq, last: Seq },
    Replay(ReplayFailure),
}

/// Runs ON the line, synchronously, during publication. Not async: cannot await.
/// Returns (): cannot report failure. Given no Session: cannot re-enter.
/// Borrows the publication: must clone the Arcs it wants, which is "capture
/// already-immutable state".
pub trait CommitObserver: Send + Sync + 'static {
    fn observe(&self, p: &Publication<'_>);
}
```

`SessionMut` lives in exactly one place — an actor loop that is the imperative shell — and arbitrary
subsystems hold a clonable `Committer` ticket that sends a job and awaits a `CommitReply` with no
handle in any variant:

```rust
while let Some(job) = rx.recv().await {
    match session_mut.commit(&job.cx, job.change).await {
        CommitOutcome::Committed { result, seq, session } => { session_mut = session; /* reply */ }
        CommitOutcome::Uncertain(u) => {
            // `session_mut` was MOVED into commit and not returned. There is nothing
            // to put back; the loop cannot continue even if someone wanted it to.
            // rx closes on drop, so every later ticket call fails.
            return;
        }
        /* NothingToCommit, RolledBack: the handle comes back */
    }
}
```

**Guarantee gained.** Reusing a poisoned Session is a **compile error inside the kernel**: the
handle is in the success variants, not beside the error, so `?` cannot smuggle one out and a
catch-and-continue has nothing to continue with — §12:4581's request becomes a rule. A backend
**cannot report a failure without choosing a class**, because the signature has no third option. The
dangerous misclassification is **unspellable**: with `RejectedReason` closed, no string arm and no
`From<io::Error>`, an `ENOSPC`, a torn write or a timeout has exactly one path, and claiming an
uncertain failure is recoverable requires inventing a variant that does not exist. Two concurrent
commits are not expressible — one non-`Clone`, non-`Arc` handle, consumed by `commit` — so
§10:4321's licence for backends to skip a commit mutex stops being an unverifiable assumption and
becomes the reason `Storage::commit(&mut self, ..)` type-checks. `NothingToCommit` is a named
variant, so no caller can order a commit that never happened against a fabricated sequence. And all
three of §4's observer prohibitions are unrepresentable rather than documented.

**Guarantee not gained.** Whether a backend's `Rejected` claim is **true** — a `Rejected`
constructed after a partial durable write compiles and lies; that is a fault-injection obligation.
Whether the batch actually committed in the `CommitStateUnknown` case; the host must reopen and
reconcile, and `UncertainKind` says only which side of the fence it died on. **At the `Committer`
boundary the guarantee is `checked`, not consuming, and this document says so rather than claiming
the consuming form while shipping an `Arc`:** a ticket cannot be consumed, so its holder learns the
Session is dead from a closed channel. That is weaker and the failure is weaker too — a ticket
holder cannot reach adoption, publication or any in-memory baseline, so it can receive an error but
cannot cause divergence. That the line is held *continuously* remains a property of the actor's
straight-line code. A panic in a line observer still unwinds — `Fn -> ()` forbids `Result`, not
`panic!` — and `[profile.release]`'s `panic = "abort"` (`Cargo.toml:447`) means a release build dies
instead of partially advancing, which removes pi's partial-advance hazard degenerately rather than
solving it; the all-or-nothing publication obligation must therefore be tested in a debug build.

**Migration cost.** Moderate and front-loaded. Every call site is a four-arm `match` rather than a
`?`. That verbosity is the point — it sits exactly where the code must decide what to do when the
Session dies. One ergonomic helper is worth it, and must not be able to produce a `SessionMut` from
the `Uncertain` arm:

```rust
impl<R> CommitOutcome<R> {
    pub fn or_fatal(self) -> Result<(R, Option<Seq>, SessionMut), UncertainCommit>;
}
```

**Benefit versus ceremony.** High benefit, low ceremony: four variants and two enums replace a
poison flag, an `instanceof` test and a documented request. **Confidence: high** on the outcome
enum and the closed `RejectedReason`; **medium** on the actor/ticket split, which is the one
architectural commitment here (mitigated by cyrup already running this shape in
`cyrup-session-svc`'s session actor and `cyrup-modes/src/rpc`, and by ADR-0028 §F1/F2 arriving at the
same shell-owns-the-channel conclusion).

---

### [P1] F2 — `Tx<Reading> → Tx<Writing>`, and a draft that borrows the transaction

**Location.** Upstream: `transaction.ts` — `#read`/`#write` and `#hasTableWrite` (`:918-936`),
`#sealed` and `#assertOpen()` (`:663, 673, 898-900`), `settleSuccess`'s *"Session commit callback
settled before its pending Tx operations"* (`:672-678`); `errors.ts:4-9`'s `ReadAfterWrite`; Chord's
draft-revoking `Proxy`. Spec: §4's table rules (`:1553-1557`), §12's *Detached draft work*
(`:4454-4457`), *Read after write* (`:4458-4459`), *Long transactions* (`:4510-4512`).

**Current representation.** A monotone boolean plus a thrown error class for the ordering; a seal
flag plus a revoked `Proxy` plus a per-access assertion for the lifetime; and, for effects and
nested commits, nothing at all.

**The legal sequence.** Within one commit, every table row the transaction needs is read **before**
its first table write; after the first table write, table reads are not available — and are not
needed, because every creation returns the record or id it created (`spec.md:1557`). Document access
and read-your-writes survive the first table write (`spec.md:1556`); the Session's own
task-document validation against a task candidate is explicitly **not** a caller table read
(`spec.md:1240-1241`). When the callback settles, every draft stops working.

**Concrete failure modes.** A harness turn appends the assistant entry, then reads the task record
to decide the next phase — natural code, because the task's phase is what the next statement
branches on. pi throws `ReadAfterWrite` mid-transaction from deep inside harness logic, the whole
commit rolls back, the turn fails, and the user watches the assistant's message vanish. The correct
order is not locally obvious, which is why §12 has to state it. Second: a helper does
`let d = tx.doc(..).await?;` and returns `d` to a caller that writes to it after the commit — pi's
own example, `escaped!.generation = undefined` (`spec.md:1368-1381`). The write is lost, the caller
believes the document changed, and the next commit overwrites. Third: a hook helper
`record_usage(session, ..)` calls `session.commit(..)` and someone calls it from inside a commit
callback; pi deadlocks the Session **permanently with no diagnostic**, because the nested call
queues behind the current job on `#tail`.

**Recommended pattern.** Typestate — two states, consuming — plus negative bounds that do most of
the work.

```rust
pub struct Reading(());          // sealed: private field, no third state
pub struct Writing(());

/// `'tx` is INVARIANT (that is what `PhantomData<&'tx mut ()>` buys), so the borrow
/// cannot be widened. Not Send, not Sync, not Clone, not 'static. It holds
/// `&mut dyn Storage` and the in-flight document set — and NO Session handle and
/// NO effect capability.
pub struct Tx<'tx, S> {
    inner: &'tx mut TxInner<'tx>,
    _state: PhantomData<S>,
    _not_send: PhantomData<*const ()>,
    _invariant: PhantomData<&'tx mut ()>,
}

impl<'tx> Tx<'tx, Reading> {
    pub async fn conversation(&mut self, id: ConversationId) -> Result<Option<ConversationRecord>, TxError>;
    pub async fn entry(&mut self, id: EntryId) -> Result<Option<EntryRecord>, TxError>;
    pub async fn task(&mut self, id: TaskId) -> Result<Option<TaskRecord>, TxError>;
    pub async fn submission(&mut self, id: SubmissionId) -> Result<Option<SubmissionRecord>, TxError>;
    /// Consuming, and taken whether or not the write that follows succeeds — which
    /// is pi's behaviour exactly (`#write` sets `#hasTableWrite` before writing).
    pub fn writing(self) -> Tx<'tx, Writing>;
}

/// BOTH states — §12:4458's deliberate asymmetry.
impl<'tx, S> Tx<'tx, S> {
    pub async fn doc<'d, D: DocToken>(&'d mut self, token: &D, scope: Scope)
        -> Result<Draft<'d, D::Value>, TxError>;
    pub async fn mint<K: IdKind>(&mut self) -> Result<Id<K>, TxError>;
}

impl<'tx> Tx<'tx, Writing> {
    pub async fn append_entry(&mut self, ..) -> Result<EntryId, TxError>;
    pub async fn create_task(&mut self, ..) -> Result<TaskId, TxError>;
    // no conversation(), no entry(), no task(), no submission().
}

/// The callback receives `Tx<Reading>` by value and must hand back a `Tx<Writing>`,
/// so the transition is how the transaction gets home rather than optional bookkeeping.
/// A callback that writes nothing calls `writing()` as its last statement; it is
/// infallible and free.
pub async fn commit<R, F>(self, cx: &Cx, change: F) -> CommitOutcome<R>
where
    F: for<'tx> AsyncFnOnce(Tx<'tx, Reading>) -> Result<(R, Tx<'tx, Writing>), CallbackError>;
```

**Guarantee gained.** For three of the four hazards this is **unrepresentable, not merely
typestate**, and the corresponding pi machinery is deleted rather than ported:

- *Escaped draft.* `Draft<'d, _>` borrows `Tx<'tx, _>` which borrows for the closure's lifetime; it
  cannot be stored in anything that outlives the callback, cannot be sent, cannot be cloned. The
  revoking `Proxy`, the `#sealed` flag and `#assertOpen()` on every draft access and every `Tx`
  method all go.
- *Fire-and-forget `Tx` work.* A future that borrows the transaction is not `'static`, so it cannot
  be spawned; the only way to make progress with it is to await it in scope. pi's *"settled before
  its pending Tx operations"* check and its pending-operation drain become unnecessary. `Tx` being
  `!Send` additionally prevents spawning the whole callback future.
- *Nested commit deadlock.* The callback is handed only the transaction, so there is no handle
  through which a nested commit can be started — the hazard pi does not even diagnose is closed
  completely, and with it every effect reachable *through* the transaction.

For read-after-write it is **typestate**: the four table readers do not exist on `Tx<Writing>`, so
`ReadAfterWrite` as an error class is deleted. It is affordable precisely because §4:1557 guarantees
creations return their ids. A `DocHandle` branded with the generative `'tx` additionally makes
cross-transaction handle confusion unrepresentable — a hazard pi has no concept of.

**Guarantee not gained.** **Invariant 4 is `guarded`.** Withholding the effect capability closes the
intended path; a closure capturing an `Arc<ModelClient>` or a `tokio::process::Command` from its
environment still compiles and still awaits inside the held line. No Rust construct forbids that.
The additional measure is runtime and is labelled so: a debug/test-build line-hold timer that fails
the suite past a budget. A draft's *contents* can still be cloned out, which is harmless and is the
intended way to carry a value out — but interior mutability or a smuggled `Arc<Mutex<_>>` in the
value model would reopen the escape in spirit, which is why F4's no-interior-mutability rule is
load-bearing here too. Nothing here says anything about reads from *other* transactions or about the
staleness of what was read. And one capability is genuinely narrowed: pi lets a caller hold a draft
across an await and interleave `tx.task()` reads with draft writes; here the caller reads first and
edits after. Every pi program has a mechanical rewrite, so no capability is lost, but call sites
move — stated rather than hidden.

**Migration cost.** Moderate, concentrated in one signature. The `for<'tx> AsyncFnOnce` bound with a
`Tx` returned by value needs async closures (stable since 1.85) and will produce lifetime errors
that are hard to read when a callback tries to hold the `Tx` across a spawn — which is the error we
*want*, with a poor message. Mitigate with a doc example of the error and its fix. The simpler
fallback, `F: for<'tx> AsyncFnOnce(&'tx mut Tx<'tx, Reading>) -> ..`, loses the consuming transition
and needs an internal flag again; it is recommended **against**, and validating the primary form
against a real borrow checker is slice S0 of the plan.

**Benefit versus ceremony.** Two marker types with private fields, and a bound. It passes every
clause of `RUST-DESIGN-REVIEW.md:40-47` — two states, finite and stable, compiler-visible at every
program point, not driven by external events, and the API is clearer because the illegal method is
simply absent. **Confidence: high** on the typestate; **medium** on the exact callback signature
until S0 lands.

---

### [P1] F3 — A keyed batch, so the five illegal states stop being expressible

**Location.** Upstream: the `StorageWrite[]` type; `memory.ts:689-695` (global id checks) and
`:716-759` (document-action checks); SQLite's `checkDocumentActions()`; `transaction.ts`'s
`#assemble()`. Spec: §10:4362-4367, §10:4274-4275, §10:4321-4323.

**Current representation.** A flat `readonly StorageWrite[]` — a sequence of independent writes —
over which §10 imposes rules the representation cannot express, so **all three backends re-validate
every one of them by hand**: *"Document N has more than one content command"*, *"is retired more
than once"*, *"delta has no base"*, *"version transition requires a base"*, *"address already has a
current incarnation"*, *"ID N already belongs to…"*, *"is written more than once"*, *"is written as
two record types"*.

**The invariant.** One commit does at most one thing to one incarnation's content, plus optionally
retire it; content is applied before retirement regardless of how the batch was assembled; a
creation carries a complete base; one number belongs to one record of one type; and there is exactly
one committer.

**Concrete failure mode.** A task finishes, and in one commit its outcome changes its task
document's final content and retires it — §3's *"retiring an acquired draft persists its final
content before retirement"* (`spec.md:1246-1247`). A straight port assembles
`[document.change(d), document.retire(d)]` on one backend and `[document.retire(d),
document.change(d)]` on another; both are legal arrays, and the second backend's hand-written
normalisation has a bug in one arm. The retire wins, the final content is discarded, and the user
loses the last state of exactly the document they care about most — reproducing on one backend only,
which §11's own framing calls the most expensive kind of bug a durable system can have. A second,
quieter mode: `assemble()` pushes a `document.change` in a loop over open changes, the loop runs
twice for one incarnation after a memoisation bug, and the effect depends on array order.

**Recommended pattern.** Explicit domain enum over a keyed structure. Not typestate on assembly
(§5 rejects `Batch<Open|Sealed>`).

```rust
/// One admitted commit. BTreeMap — not HashMap, not IndexMap — so iteration is
/// deterministic by id and the JSONL marker and SQL statement order are
/// byte-reproducible, which the conformance suite depends on.
pub struct Batch {
    conversations: BTreeMap<ConversationId, ConversationRecord>,
    entries:       BTreeMap<EntryId, EntryRecord>,
    tasks:         BTreeMap<TaskId, TaskRecord>,
    submissions:   BTreeMap<SubmissionId, SubmissionRecord>,
    documents:     BTreeMap<DocumentId, DocumentCommand>,
}

/// What ONE commit does to ONE incarnation. Content and retirement are one value,
/// so there is no order between them to override and no way to say "content twice"
/// or "retire twice".
pub enum DocumentCommand {
    Create { record: DocumentCreate, base: DocumentBase, then: Retire },
    Copy   { record: DocumentCreate, source: CopySource,  then: Retire },
    Change { content: DocumentContent,                    then: Retire },
    RetireOnly,
}
#[derive(Clone, Copy)] pub enum Retire { Keep, Retire }

/// Note the type: a creation carries a BASE, never a DocumentContent, so
/// "a delta with no base" is not a shape that exists.
pub struct DocumentBase { pub version: DefVersion, pub value: DocRoot }

pub enum DocumentContent {
    Base(DocumentBase),
    /// A delta extends the base it was prepared against. `continues` is not a number
    /// the assembler picks — it is F6 §C's unforgeable StoredVersion witness.
    Delta { continues: StoredVersion, ops: OpBatch },
}

#[async_trait]
pub trait Storage: Send {
    /// `&mut self`: §10:4321's "storage implementations do not add a second
    /// caller-facing commit mutex" stops being an assumption no type states.
    async fn commit(&mut self, batch: Batch, cx: &Cx) -> Result<Seq, CommitError>;
    async fn mint<K: IdKind>(&mut self, cx: &Cx) -> Result<Id<K>, CommitError>;
    /// Read paths stay `&self`.
    async fn document(&self, id: DocumentId, at: DocumentPoint, cx: &Cx)
        -> Result<Option<StoredDocument>, StorageFailure>;
}
```

**Guarantee gained.** Four of the five illegal states become unspellable and the hand-written
validation is **deleted in three places rather than ported**: two content commands for one
incarnation (one map value), two retirements (`Retire` is not a count), a delta with no base
(`Create`/`Copy` take `DocumentBase`), and any dependence on write-array order (there is no array).
§10:4363's rule — imposed on a representation that *has* an order — stops needing to be imposed, and
the per-backend normalisation pass disappears with it. Within-table id duplication is unspellable
(one map key). And `commit(&mut self, ..)` with the handle owned exclusively by F1's `SessionMut`
makes a second concurrent committer a compile error.

**Guarantee not gained.** Two of `memory.ts`'s checks are **cross-batch** facts and stay runtime
checks in the backend: *"address already has a current incarnation"* and global id ownership against
already-committed records. *"Version transition requires a base"* is partly lifted by F6 §C's
witness and partly cross-batch. **Atomicity in the medium is untouched**: one owned `Batch` means
the Session cannot split a logical change in two, and nothing stops a backend partially applying it
— that is conformance plus fault injection. Order **within** one incarnation's content is still
semantic and must survive, because a delta tail is ordered (`spec.md:4350`), which is why `OpBatch`
stays a sequence inside `Delta`. Cross-record semantic validity — ancestry, references, transitions
— is explicitly the Session's job (`spec.md:4273-4274`) and stays so.

**Migration cost.** Low on the kernel side; the assembler builds maps instead of pushing to a vec,
which is slightly *simpler* code. Real on the backend side: each apply loop iterates five maps
instead of matching one array. Since no backend exists in cyrup today, the cost is **zero now and
large if deferred** — which is the whole sequencing argument for landing the trait before any engine.

**Benefit versus ceremony.** The best ratio in this document: five map fields and a four-variant
enum delete three copies of eight checks. **Confidence: high.**

---

### [P1] F4 — One owned, `Arc`-shared immutable document value

*The highest-value finding; §4 is its before/after.* **Location.** Upstream: Chord's
`src/delta/README.md` mutation-rights table and its opening paragraph; `memory.ts:92-100`'s
recursive `clone` with the comment at `:216`; `transaction.ts`'s `copyJson()` at `:366, 407, 433,
470, 606`. Spec: §1 invariant 6 (`:72`), §3.4 (`:1340-1369`), §4's storage ownership (`:1560-1566`),
§11.1 (`:4375-4378`), §11.3's warning (`:4402-4405`), §12's *Trusted immutable revisions* (`:4528`).

**Current representation.** A convention, with the consequence stated outright:

> Immutability is an ownership contract. Nothing is frozen or defensively copied, so an illegal
> mutation is not detected. It silently corrupts state.

and a table of ten rows about which values may be mutated by whom, backed by deep copying at every
boundary, which §11.1 admits exists only to *simulate* the detachment SQLite and JSONL get free.

**The invariant.** Every published revision, every operation payload and every selected base is
immutable for all time; successive revisions share unchanged subtrees structurally; a value assigned
into a draft is copied at the assignment and the caller may go on mutating its own object; only
strict JSON enters a document, rejected at the offending assignment before the draft changes; and
storage never aliases caller memory in either direction.

**Concrete failure modes.** Two, and the second is **verified from source rather than asserted**.

*Aliased revision.* A TUI mount holds `state.value` from `documentState(LiveDoc, c)` and a renderer
sorts `value.tools` in place to display them ordered. `tools` is a tracker-owned container shared
with the authority revision and with every other observer's retained revision. §12 says plainly that
runtime freezing is not provided, so nothing throws: the committed revision every other mount and
every replica computed from is now reordered, with no write, no commit and no event to attribute it
to. Equivalently on the storage side, the JSONL backend caches materialised current values — the
obvious optimisation, since re-replaying a base plus forty deltas per read is expensive — and
returns a handle sharing the map's container.

*`NaN` becomes `null`, silently.* A tool records a token-cost average in its task document, the
divisor is zero for an empty turn, and `f64::NAN` is assigned. In `serde_json` 1.0.150:

```
src/value/ser.rs:156   fn serialize_f64(self, float: f64) -> Result<Value> { Ok(Value::from(float)) }
src/value/from.rs:59   fn from(f: f64) -> Self { Number::from_f64(f).map_or(Value::Null, Value::Number) }
src/number.rs:183      pub fn from_f64(f: f64) -> Option<Number> { if f.is_finite() { … } else { None } }
```

`serde_json::to_value(f64::NAN)` returns **`Ok(Value::Null)`**. It does not error. The commit
succeeds, the publication carries `null`, the UI shows a blank, and on reopen the field is
permanently `null` — the information is gone and no error was ever raised. pi catches this at the
assignment because Chord's copy walk rejects non-finite numbers; a straight Rust port using
`to_value` does not. The read direction is safe by construction: `from_str("NaN")` errors, so the
reopen path cannot produce one, and **that asymmetry is why the parse boundary must be a custom
`Serializer` rather than a validating `Deserialize`** — the opposite of where
`RUST-DESIGN-REVIEW.md:73` usually points.

**Recommended pattern.** Newtype at the boundary plus functional core; emphatically **not**
`serde_json::Value` as the stored type.

```rust
/// The persisted document value. Containers are Arc'd so revisions share structure.
/// No variant contains Cell, RefCell, Mutex, OnceCell or UnsafeCell — that is this
/// type's load-bearing property, and it is what makes `Arc<DocValue>` genuinely immutable.
#[derive(Clone, PartialEq, Debug)]
pub enum DocValue {
    Null, Bool(bool),
    Num(JsonNum),                 // = serde_json::Number; non-finite UNREPRESENTABLE
    Str(Arc<str>),
    List(Arc<Vec<DocValue>>),
    Map(Arc<DocMap>),             // DocMap = IndexMap<Arc<str>, DocValue>
}

/// A document root is a JSON object, so `Value::String` as a root is unrepresentable.
pub struct DocRoot(Arc<DocMap>);  // private field

impl DocValue {
    /// The sole construction path from host data. A custom Serializer whose
    /// serialize_f64/serialize_f32 reject non-finite instead of substituting null.
    /// There is no `From<serde_json::Value>` and no `From<f64>` that skips it.
    pub fn parse<T: Serialize + ?Sized>(value: &T) -> Result<DocValue, NotStrictJson>;
}

/// Structural sharing and the applier are the same mechanism: clone the authority
/// Arc (refcount >= 2 by construction), then walk the mutation path with
/// Arc::make_mut, cloning exactly the spine and sharing every untouched subtree.
fn at_mut<'a>(root: &'a mut DocValue, path: &Path) -> Result<&'a mut DocValue, PathError>;

/// Commit takes the batch BY VALUE: a backend that keeps anything already owns it,
/// and there is no caller memory left to alias. Reads return owned or Arc<immutable>
/// values — never a reference into the backend's decoded index or cache.
pub struct StoredDocument {
    pub record: DocumentRecord, pub version: DefVersion,
    pub value: Arc<DocRoot>, pub deltas_since_base: u32,
}
```

**Guarantee gained.** Mutation of a published revision or any retained descendant is a compile error
rather than a convention: `Arc<DocValue>` offers no `&mut`, `Arc::get_mut` requires uniqueness, and
`Arc::make_mut` clones when shared — which *is* copy-on-write and is correct. Chord's entire
mutation-rights table and its *"don't mutate anything the tracker owns"* rule stop being rules. The
**alias-free-root requirement vanishes**: an `Arc` subtree placed at two keys is correct because it
is immutable, so pi's provenance bookkeeping over which roots are admissible is unnecessary and any
owned `DocValue` is a valid root, taken in O(1) with no traversal. §4's whole storage-ownership
section and `memory.ts`'s recursive copy on both paths have **no Rust equivalent to write** —
`CYRUP-DELTA`: same guarantee, mechanism deleted. Eight of Chord's ten rejected placement shapes
(`undefined` members, functions, symbols, bigints, accessors, symbol keys, sparse arrays, foreign
prototypes) are not representable and need no check, which is what **licenses keeping pi's own
optimisation** — no strict-JSON walk of prepared operations or selected bases (`spec.md:1348-1351`)
— rather than re-earning it. Because the mutator takes an already-parsed `DocValue`, pi's
*"`push(valid, invalid)` inserts nothing"* is not a rule to implement but the shape of a function
taking a wholly-parsed vector. Deep equality for the no-op rules comes free with pi's exact
semantics: `IndexMap`'s `PartialEq` is order-insensitive (`indexmap-2/src/map.rs:1869-1881`),
matching Chord's *"equality ignores key order"*, while `serde_json/preserve_order` — already enabled
workspace-wide at `Cargo.toml:192` — keeps insertion order in the bytes. And `prepare()` has nothing
left to compute, because the candidate was built incrementally as the ops were recorded.

**Guarantee not gained.** Five things, each a real test obligation. (1) The host's own `Serialize`
impl is a construction path and can produce a non-finite float, so `DocValue::parse` **is** a
runtime check that must exist and must have a named negative test for the `NaN` case — that test is
the artefact that stops someone reintroducing `to_value`. (2) `parse` must accept `impl Serialize`
and must **not** also offer a path taking a pre-built `serde_json::Value`; if it ever does, that path
needs the equivalent walk. (3) `Serialize` over a cyclic `Rc` graph recurses without bound;
`parse` needs a depth limit and a test, since `#![forbid(unsafe_code)]` does not protect against a
stack overflow. (4) The freeness of the ownership deletion is **conditional on a signature choice**
— it holds only if reads return owned or `Arc<immutable>` values, never a reference into a cache; a
backend author wanting `&DocValue` from a cache hits the `async fn`-in-trait lifetime wall and will
reach for a workaround, so *"return `Arc`, never borrow from cache"* and *"no `#[serde(borrow)]`, no
`Cow` in persisted record types"* are rules that belong in this ADR, not inferences. (5) `Arc`
sharing of an *immutable* value is correct and desirable — it is exactly what §4:1566's *"immutable
strings may be shared"* licenses — so the boundary enforced is *no shared **mutable** container*,
which rests entirely on the no-interior-mutability rule. A future `Arc<Mutex<_>>` inside a host
document type would silently undo it, and that is the one thing a reviewer must check. None of this
says the value is *semantically* right; a document whose field holds a stale message is perfectly
strict JSON.

**Migration cost.** Zero for the deletions — it is work not done. Roughly 400–500 lines for
`DocValue`, `DocRoot`, the `parse` serializer and `at_mut`, all pure and unit-testable with no
runtime. **Confidence: high**, and the `NaN` behaviour is read from the dependency's source, not
recalled.

---

### [P1] F5 — A witness chain from durability to publication

**Location.** Upstream: `session.ts` `#runCommit`'s single `#publish()` call site; `tx.adopt(seq)`.
Spec: §1 invariants 2, 3 and the ordering half of 7; §13:4591's non-goal.

**Current representation.** *"There is only one call site, placed after the commit resolved."*
Nothing in the type system prevents a second.

**Concrete failure mode.** The live view and the TUI both want to render a tool-call result as it
streams, and the throttle window is 100–200 ms, so the UI feels laggy. The natural optimisation is a
`session.publish_provisional(..)` or an extra `tx.notify(..)` that emits before the commit lands.
pi's design cannot reject it — the guarantee rests entirely on there being no other emitter, which
is also exactly what §13:4591 lists as a non-goal. The second mode is a refactor that moves the
publish *above* the adopt call to "shorten the line hold": observers then receive operations computed
against a baseline memory has not swapped to, and the UI diverges from storage with nothing to
notice it.

**Recommended pattern.** Private-constructor witnesses, each consumed by the next step.

```rust
/// Crate-private. No public constructor, no Default, no Clone, no Deserialize.
/// The `_seal: ()` field is what makes a struct literal outside this module a
/// compile error — and is why `derive(Deserialize)` must never be added.
pub(crate) struct Durable    { seq: Seq, _seal: () }
pub(crate) struct Publication{ seq: Seq, changes: Arc<[Change]>, _seal: () }

impl<'tx> Tx<'tx, Writing> {
    /// Pointer-swap adoption. Consumes the transaction AND the durability witness.
    pub(crate) fn adopt(self, d: Durable) -> Result<Publication, AdoptionFailed>;
}
impl SessionShared {
    /// The ONLY publish. Takes the witness by value.
    pub(crate) fn publish(&self, p: Publication, cx: &Cx);
}

// The one place `Durable` is minted — the kernel's single Storage::commit call site:
let seq = storage.commit(batch, cx.without_abort()).await?;  // (match, per F1)
let durable = Durable { seq, _seal: () };   // the one constructor
let publication = tx.adopt(durable)?;        // consumes it
shared.publish(publication, cx);             // consumes that
```

`AdoptionFailed` carries the `seq`, because that is F1's `UncertainKind::AdoptionFailedAfterCommit`.

**Guarantee gained.** Publishing before durability is **not expressible**. A second emitter — the
optimistic/provisional channel invariant 3 and §13:4591 forbid — has no `Publication` to pass and
cannot construct one. Adoption before storage success is not expressible. The order
commit → adopt → publish is enforced by the type system rather than by one call site and a comment,
and because each witness is consumed, a publication cannot be replayed.

**Guarantee not gained.** That the publication reaches **every** observer — a throwing line observer
still truncates the iteration, which is a shell obligation (run them under `catch_unwind`; advance
all or none), not a type one. That the line was held across the whole window — that is a property of
F1's actor loop. That `adopt` is actually allocation-free and callback-free; no type proves purity,
so keep it a small `#[inline]` function and assert it with a test that counts allocations. And
`Durable` means *storage said committed* — under `Durability::ProcessCrash` that is page-cache only
(`spec.md:4420`). That half is the stated envelope, not a lie the witness tells.

**Migration cost.** Negligible — about forty lines, written once at the start. Retrofitting is the
expensive direction, because every adoption and publication signature changes. **Confidence: high.**

---

### [P2] F6 (grouped) — four supporting mechanisms

Grouped to respect the five-finding cap. **§D exceeds the cap deliberately**, on serious-correctness
grounds stated at the head of this section.

**§A — Identity.** Kind-tagged `Id<K>(NonZeroU64, PhantomData<fn() -> K>)` over a **sealed**
`IdKind` trait implemented for exactly the five kinds; a separate `Seq(NonZeroU64)`; `DocumentPoint`
as a two-variant enum; private fields, no `From<u64>`, no `Default`, no casual round-tripping
accessor; `mint<K>(&mut self)` on the storage handle as the only allocator. `NonZeroU64` because
`ROOT_CONVERSATION_ID` is 1 and 0 is never valid, so "id zero" is unrepresentable and
`Option<Id<K>>` is niche-packed for free; `PhantomData<fn() -> K>` rather than `PhantomData<K>` so
`Id<K>` stays covariant and `Send`/`Sync` regardless of `K` — get that right once.
*Gained:* seq-as-id and cross-kind confusion are unrepresentable, replacing eleven lines of erased
brands applied by bare casts (`ids.ts`). **One nuance worth stating, because the natural reading of §10:4274 is wrong:**
the cross-table half of id collision *is* liftable in-process, because with no
`From<u64>` and no public constructor a number minted as an entry cannot become a `TaskId` without
crossing a decode boundary. *Not gained:* a correct-kind-but-wrong id; monotonicity and non-reuse
across reopen; and the kind itself at a decode boundary, because **the number carries no tag** — a
record read from the entry index deserialises its `id` as `Id<Entry>` because that is the field's
type, so a corrupted file placing an entry id in the task index is accepted by the field and must
still be caught by the backend's ownership check against its committed index.
*Cost:* low, and it must be **first** — these types appear in every signature.
*Recommendation on ownership enforcement:* do **not** build a separate durable number→kind table
(§5); derive ownership from the per-table indexes, which exist anyway.

**§B — Cursors and pages.** One opaque cursor type per scan (`EntryCursor`, `ConversationCursor`,
`TaskCursor`, `SubmissionCursor`, `DocumentCursor`) over a backend-private `CursorBytes { store:
StoreId, payload: Box<[u8]> }` with no public constructor and **no `Deserialize`**; plus
`PageLimit(NonZeroU32)` with a `MAX` and a parsing constructor, and `Page<T, C> { items, next:
Option<C> }`.
*Gained:* cross-scan reuse does not compile, replacing one structural bag shared by every scan and
every backend; cross-store reuse is caught by a `StoreId` compare. That compare is easy to dismiss
as ceremony — it is one `u128` comparison, and the failure it prevents is a silent hole in a user's
transcript, so it is recommended rather than dropped.
*Not gained:* nothing forces a backend to use an index (query-plan and benchmark suites do that);
and withholding `Deserialize` means a host cannot persist a cursor across restarts, which is
deliberate — §10 does not require it, and adding it would need a validating parse plus pi's shape
check (`memory.ts:123`).

**§C — Corruption is not absence.** `Result<Option<T>, StorageFailure>` with
`StorageFailure::{Corrupt(Corruption), Io(..), HistoryNotRetained { document }}`; a `Lifetime`
newtype whose `new` rejects an inverted interval while keeping the empty one legal, with `contains`
the only membership accessor; and a `ReplayPlan::parse(&[StoredContent]) -> Result<ReplayPlan,
Corruption>` that is the single home of the two corruption rules — missing required base, and a
version change inside a delta tail — so `materialize(&ReplayPlan) -> DocRoot` is a **pure, total**
function. A `StoredVersion` witness minted only by a storage read means a `Delta { continues }`
cannot claim a version it did not read.
*Gained:* the two channels cannot be confused by taking a different code path, which is pi's only
separation; the `<=`/`<` asymmetry has one implementation instead of two; replay is directly
unit-testable with literal record sets and no storage.
*Not gained:* whether the records handed to the parser are *all* of them — a torn tail, a lost
sidecar or an unauthorised reclamation presents as a legitimately shorter set, so JSONL recovery,
reclamation ordering and marker survival stay runtime obligations.

**§D — An exclusive store lock, and a durable rename.** Neither the specification nor pi's
implementation states these requirements, and cyrup needs both.

```rust
/// RAII proof that this process holds the store exclusively. A writable backend
/// cannot be constructed without one. Not Clone.
pub struct StoreLock { _file: File, path: PathBuf }
impl StoreLock { pub fn acquire(dir: &Path) -> Result<Self, StoreBusy>; }

impl JsonlStore {
    pub fn open_for_write(dir: &Path, lock: StoreLock, opts: JsonlOptions) -> Result<Self, StorageFailure>;
    /// No lock, and no `commit`.
    pub fn open_read_only(dir: &Path) -> Result<ReadOnlyStore, StorageFailure>;
}
/// §11.3's two tiers, NAMED rather than a bare bool, because `fsync: false` is a
/// durability CLAIM and should read like one at the call site. pi defaults it to
/// false (`jsonl/storage.ts:77-78`).
pub enum Durability { ProcessCrash, PowerLoss }
```

*Concrete failure mode, and it is specific to cyrup rather than to pi:* **cyrup is a CLI.** A user
runs `cyrup` in a repository, leaves it open, and runs a second `cyrup` in the same repository in
another terminal. `cyrup-session` tolerates that today by design — its reader skips malformed lines
and last-good-line wins — because it has no cross-record invariants to break. Under Pico5 both
processes mint ids from their own durable high-water mark and both allocate commit sequences. Within
seconds two records share id 41, a fork's `parent.at` points at the wrong entry, and two
incarnations' half-open lifetimes overlap. Nothing errors; the store is silently incoherent and the
next reopen publishes a plausible, wrong history.

*Second failure mode, already latent in the tree:* `crates/cyrup-session/src/store.rs:322-324` does
`f.sync_data()?` then `std::fs::rename(&tmp, &self.path)?` with **no parent-directory fsync**, so
the rename is not durable on unix. §11.3's reclamation path renames a sidecar replacement over a
live sidecar; on power loss the directory entry never reached the device, the old sidecar is back,
and the authorising marker survived — which is exactly the *marker without its data* corruption
§11.3:4425 exists to prevent.

*Gained:* a writable backend cannot be built without the proof, so "forgot to lock" is a compile
error. The lock also makes a mechanism improvement sound that is otherwise not: because this process
is the only appender, it knows each sidecar's byte offset before writing the marker, so **the marker
can carry those offsets** and the open pass never reads sidecar payloads at all. That attacks the one
requirement a file backend must re-implement rather than borrow (the indexed access paths), and turns
`document(id, at)` into one seek plus the tail — a `CYRUP-DELTA`: same §10 guarantee, cheaper
mechanism than a marker that merely lists records.
*Not gained:* an advisory lock is advisory. A process that does not take it still writes; `flock` is
unreliable or absent on NFS and some network filesystems; delete-and-recreate of the lockfile under a
held lock is possible on unix. So this is **guarded**, and the store should also carry a `StoreId`
plus a holder-pid record so a stale or ignored lock is detectable. It says nothing about a second
process *reading* while this one writes, which is legal and desirable and which the marker protocol
already makes safe. The Windows arm of `durable_rename` is an open question (§14), not a solved
problem. SQLite would supply all of this through its own locking, which is a genuine point in the
engine's favour and is recorded in §9 rather than hidden here.
*Cost:* one small dependency — `fs4`, pure Rust over `libc`/`windows-sys`, chosen over `nix` because
`docs/adr/ADR-0007-windows-scope.md` notes `nix` compiles to an empty crate on the Windows target —
plus roughly sixty lines and a `durable_rename` helper that `cyrup-session`'s `rewrite` should also
adopt.

---
## 4. Highest-value refactor sketch — F4, the document value

### Before — pi `v1.0.0`

The guarantee is a sentence. `packages/chord/src/delta/README.md`, first paragraph after the title:

```
Immutability is an ownership contract. Nothing is frozen or defensively copied,
so an illegal mutation is not detected. It silently corrupts state.
```

It is then operationalised as a ten-row table of who may mutate what, of which these four rows are
the load-bearing ones:

```
| Root passed to track(), prepareReplace(), replicatedState(), or replace() | No. Ownership moved to the tracker. |
| change.state and handles read from it | Yes, only while that change is open. After prepare(), abort(), or another adoption, every use throws. |
| External value after assigning or inserting it into a draft | Yes. The draft stored a validated clone. |
| tracker.value, prepared.base, retained older revisions | No. |
```

and backed, in the storage layer, by a hand-written recursive copy on **both** the write and the
read path. `packages/durable/src/storage/memory.ts:92-100`:

```ts
const clone = <T>(value: T): T => {
	if (value === null || typeof value !== "object") return value;
	if (Array.isArray(value)) return value.map((item) => clone(item)) as T;
	const source = value as Record<string, unknown>;
	const nullPrototype = Object.getPrototypeOf(value) === null;
	const result = (nullPrototype ? Object.create(null) : {}) as Record<string, unknown>;
	for (const key of Object.keys(source)) {
		const copied = clone(source[key]);
		/* … prototype-pollution guard via defineProperty … */
```

whose purpose the file states at `:213-218`:

```ts
/**
 * Detached in-memory reference implementation of `Storage`.
 *
 * Reads and retained writes are cloned intentionally to match the ownership boundary
 * of serialization-backed stores. This is backend conformance, not validation.
 */
```

And the draft surface is a `Proxy` over ordinary property assignment, revoked at `prepare()`:

```ts
const change = tracker.beginChange();
change.state.output += "done\n";            // mutable only while the change is open
change.state.entries.push({ id: 1 });        // placed values are cloned
const prepared = change.prepare();           // draft handles are unusable from here on
```

So pi pays, at runtime, for: a revocation proxy, a per-access seal assertion, a validating copy walk
at every placement, `copyJson()` on every caller root and every table record
(`transaction.ts:366, 407, 433, 470, 606`), a recursive clone on both storage paths, and a prose
contract for the part none of that can reach. §4:1560-1566 is an entire specification section about
ownership, and §11.3:4402-4405 has to warn implementers that *"a JSONL backend cannot simply add
file appends around aliasing memory tables."*

### After — proposed `crates/cyrup-pico-doc/src/value.rs`

```rust
/// The persisted document value. Containers are `Arc`'d so successive revisions
/// share every unchanged subtree.
///
/// LOAD-BEARING: no variant contains `Cell`, `RefCell`, `Mutex`, `OnceCell` or
/// `UnsafeCell`. That, and only that, is what makes `Arc<DocValue>` immutable
/// rather than merely shared. Adding an interior-mutable variant — or admitting a
/// host value type that contains one — reopens every hazard this type closes.
#[derive(Clone, PartialEq, Debug)]
pub enum DocValue {
    Null,
    Bool(bool),
    /// `serde_json::Number`: `from_f64` returns `Option`, so a non-finite value is
    /// unrepresentable *inside* the tree (`serde_json-1.0.150/src/number.rs:183`).
    Num(JsonNum),
    Str(Arc<str>),
    List(Arc<Vec<DocValue>>),
    Map(Arc<DocMap>),
}
pub type DocMap = IndexMap<Arc<str>, DocValue>;

/// A document root is a JSON object. `DocValue::Str` as a root is unrepresentable.
pub struct DocRoot(Arc<DocMap>);   // private field; built only via DocRoot::parse

impl DocValue {
    /// The sole construction path from host data, and the one real runtime check
    /// left in this design. A `serde::Serializer` modelled on serde_json's own
    /// `value::Serializer`, differing in exactly two methods:
    ///
    ///   fn serialize_f64(self, v: f64) -> Result<DocValue, NotStrictJson> {
    ///       if !v.is_finite() { return Err(NotStrictJson::NonFinite(v)); }
    ///       Ok(DocValue::Num(JsonNum::from_f64(v).ok_or(..)?))
    ///   }
    ///
    /// `serde_json::to_value` MUST NOT appear on the document write path: its
    /// `serialize_f64` is `Ok(Value::from(float))` (`src/value/ser.rs:156`), and
    /// `From<f64> for Value` is `Number::from_f64(f).map_or(Value::Null, ..)`
    /// (`src/value/from.rs:59`) — a `NaN` becomes a silent `null`.
    pub fn parse<T: Serialize + ?Sized>(value: &T) -> Result<DocValue, NotStrictJson>;
}

/// Structural sharing and the applier are one mechanism. The change clones the
/// authority `Arc` (so its refcount is >= 2 by construction), then walks the
/// mutation path with `Arc::make_mut`: exactly the containers on the spine are
/// cloned, every untouched subtree is shared. This IS `applyImmutable`'s
/// "unchanged subtrees are structurally shared with base" — as a refcount
/// consequence, not as code.
fn at_mut<'a>(root: &'a mut DocValue, path: &Path) -> Result<&'a mut DocValue, PathError>;

/// A draft borrows its transaction (F2), so it cannot escape. Mutation is
/// path-addressed because Rust has no assignable-property analogue — the one place
/// cyrup's surface is less ergonomic than pi's. CYRUP-DELTA: same operations,
/// different call shape.
pub struct Draft<'d, T> { ch: &'d mut OpenChange, _t: PhantomData<fn() -> T> }

impl<'d, T> Draft<'d, T> {
    pub fn set(&mut self, path: &Path, value: DocValue) -> Result<(), PathError>;
    pub fn delete(&mut self, path: &Path) -> Result<(), PathError>;
    pub fn append_str(&mut self, path: &Path, text: &str) -> Result<(), PathError>;
    pub fn trim_str_front(&mut self, path: &Path, n: TrimLen) -> Result<(), PathError>;
    pub fn splice(&mut self, path: &Path, at: u32, remove: u32, items: Vec<DocValue>) -> Result<(), PathError>;
    pub fn permute(&mut self, path: &Path, perm: Permutation) -> Result<(), PathError>;
    pub fn replace_root(&mut self, root: DocRoot);   // Chord's prepareReplace: O(1) ownership move
    pub fn read(&self) -> &DocValue;                 // read-your-writes inside the change
}

/// Storage takes the batch BY VALUE and returns owned or `Arc<immutable>` data.
/// Two rules make the deletion sound and belong in the ADR, not in a reviewer's head:
///   1. a read NEVER returns a reference into the backend's decoded index or cache;
///   2. no `#[serde(borrow)]` and no `Cow<'_, _>` in any persisted record type.
async fn commit(&mut self, batch: Batch, cx: &Cx) -> Result<Seq, CommitError>;
async fn document(&self, id: DocumentId, at: DocumentPoint, cx: &Cx)
    -> Result<Option<StoredDocument>, StorageFailure>;
```

### What the diff actually is

| pi mechanism | cyrup |
|---|---|
| Chord's ten-row mutation-rights table | the type. Deleted. |
| the alias-free-root rule and its provenance bookkeeping | unnecessary: an `Arc` subtree at two keys is correct because it is immutable. Deleted. |
| draft revocation `Proxy` + `#sealed` + `#assertOpen()` per access | a borrow (F2). Deleted. |
| the validating copy walk at every placement | a move of an already-parsed value. Deleted. |
| `copyJson()` at five call sites in `transaction.ts` | ownership. Deleted. |
| `memory.ts:92-100`'s recursive `clone`, on both paths | ownership. **Deleted — and this is the one §11.1 admits only simulates what Rust gives free.** |
| §4:1560-1566, an entire specification section about ownership | the signatures. Deleted. |
| eight of ten rejected placement shapes | not representable in Rust. No check at all. |
| non-finite floats rejected at the placement | **kept**, as `DocValue::parse`'s custom `serialize_f64`. The one real check that survives — and the one a naive port would lose. |
| per-incarnation structural sharing | `Arc::make_mut`. Free. |
| deep equality ignoring key order | `IndexMap: PartialEq` (`indexmap-2/src/map.rs:1869-1881`) + `serde_json/preserve_order`, already on at `Cargo.toml:192`. Free. |

**Net:** roughly 400–500 lines of pure, runtime-free Rust in exchange for one specification section,
one ten-row contract table, one recursive clone duplicated across three backends, five `copyJson()`
call sites, a revocation proxy and a per-access seal check — with one runtime check *added*, because
`serde_json` would otherwise turn a `NaN` into a `null` and nobody would find out until the next
restart.

---

## 5. Deliberately rejected opportunities

This section is not optional. It is where the document argues against applying a pattern in the
places it looks most attractive.

**Typestate on the task machine (`Task<Pending> → Task<Running> → Task<Waiting> → Task<Completing> → Task<Terminal>`).**
The single most tempting application in the whole subject, and the one that would be most wrong. A
task is *"a durable state machine"* (`spec.md:51`) whose state is a JSON `checkpoint` persisted
inside its record, reconstructed at open, held in one `TaskId`-keyed table beside every other task,
and selected by a scheduler reacting to external events; one phase-handler invocation is all the
compiler ever sees, and the real lifecycle spans process restarts. That trips four of
`RUST-DESIGN-REVIEW.md:55-61`'s five rejection clauses at once, including the last and most-missed
one. It is also mechanically impossible for the same reason ADR-0028 sent `ToolCallStream` to an
enum: *"the streams live in a `HashMap<ToolCallId, _>` and are selected by a value that arrives off
the wire, so the map needs one concrete value type"* (ADR-0028:659). **Better:** `enum TaskState`
with five variants and `enum TaskOutcome` with five, exhaustively matched, with the data valid in
each state held in that variant (`memos` on the three live variants, `outcome` on the two settled
ones — which is exactly how `spec.md:1600-1618` already shapes `TaskRecord`). The one capability-shaped
type that *is* right in §5 is narrow: an abort handler receives a context type without the
create-owned-work capability, because §12:4553 says an abort handler cannot create owned children.

**Typestate on the Session lifecycle (`Session<Open> → Session<Closing> → Session<Closed>`).**
Direct hit on the compiler-visible-lifecycle rule, and on ADR-0028's own reasoning. §4:1546-1549 and
§12:4575-4577 say close **seals admission immediately** and then joins every outstanding invocation
— so a hook that ignores its signal keeps `close()` pending and storage open indefinitely, and
§13:4601 declines to force-terminate it. A `Session<Closing>` type would encode a window whose end
the compiler cannot see and whose duration is bounded only by third-party code. It would also have
to coexist with `Session<Open>` behind the same `Arc` held by every watch and observer, forcing one
concrete type and a runtime tag anyway. **Better:** a one-way `admitting: bool` on `SessionMut`,
sealed first in `close()`, plus a memoised close future — and `SessionClosed`/`SessionDead` as
variants of `CommitReply`.

**Typestate on the storage backend (`Store<Closed> → Store<Open> → Store<Poisoned>`).** Two
independent disqualifiers. The backend is selected at runtime from configuration and held as
`Box<dyn Storage>`, so a generic state parameter would infect `SessionMut`'s own type and force
erasure back to one concrete value. And the poison state must be readable from the `&self` read
paths, which cannot consume anything — pi has exactly this (`JsonlStoragePoisonedError`,
`jsonl/storage.ts:90`) and must. **Better:** a runtime poison flag inside the backend with
`CommitError::Uncertain` as its only producer. The guarantee that *matters* — the Session not
continuing — lives one layer up in `CommitOutcome` (F1), where ownership is exclusive.

**A two-phase commit callback: an async read phase, then a synchronous write phase.** The most
tempting design in the transaction, because it would make invariant 4 nearly structural and would
give the read-before-write rule for free with no typestate at all. It is wrong, and §3 kills it
concretely: a task-scoped document acquisition validates against the transaction's own **latest
candidate** task record (`spec.md:1246-1248`), and task documents created earlier in the same commit
retire with the task — so a document acquisition legitimately **follows** a table write in the same
callback, and acquisition is async because it reads storage. A synchronous write phase either
forbids that (a behavioural concession ADR-0029 rules out) or reintroduces an await in phase two,
gaining nothing. The variant where the read phase runs **off** the line is worse: another commit
could land between the reads and the writes, so the writes would be derived from a baseline already
durably stale — the divergence the read-before-write rule exists to prevent, writ large.
**Better:** F2's one async callback with no effect capability and no Session handle, and invariant 4
labelled `guarded` with a debug line-hold budget rather than falsely labelled structural.

**A kernel-owned `EffectPlan` returned from the transaction and convertible into runnable effects
only by a successful `Committed`.** It would make "an effect ran for a rolled-back transaction"
unrepresentable — except the guarantee is **already free** from F2's negative bounds: `Tx<'tx, _>` is
non-`Send`, non-`Clone`, non-`'static`, so nothing escapes the callback except its return value `R`,
which means effects are necessarily run after the await. A named kernel type closes no reachable bad
state, and a kernel-owned registry of pending effects edges toward §13:4592's excluded
*"independently maintained event state"*. **Better:** recommend it as a *caller* pattern — the
callback returns a plain value describing what to do next, and the caller does it after the await —
and let the bounds be the enforcement.

**Typestate on batch assembly (`Batch<Empty> → Batch<Partial> → Batch<Sealed>`).** There is no
caller sequence to constrain: the batch is built in one pass by the transaction's own assembler, it
is never handed to third-party code, and every bad state typestate would guard is already unspellable
in F3's keyed form. This is the ceremony-over-protection case `RUST-DESIGN-REVIEW.md:121` names.
**Better:** F3's keyed `Batch` with a private constructor.

**Typestate or a phantom-indexed `Seq` to prove commit ordering at compile time.** `Seq` is read
from storage, crosses a process boundary, survives reopen, and is compared against values a previous
process wrote — `RUST-DESIGN-REVIEW.md:57`'s persisted-state clause rejects it outright, and nothing
a type says about an in-memory `Seq` constrains what the next reopen reads. **Better:** F6 §A's
newtype plus the backend's strict-increase check at both the write and the recovery boundary
(`memory.ts:253`, `jsonl/storage.ts:543-546`), verified by the reopen suite.

**A parallel document value tree replacing `serde_json` entirely.** A `DocValue` with a
`Finite(f64)` variant *would* make non-finite floats unrepresentable rather than guarded, which is
strictly stronger — but it costs a conversion at every boundary (draft, batch, marker, sidecar, SQL
row, publication, every host type's `Serialize`) and duplicates a parser cyrup already depends on
and already configures correctly. `serde_json::Number` already cannot *hold* a non-finite; the
hazard lives entirely in the `to_value` conversion, which is one function. **Better:** F4 keeps
`JsonNum = serde_json::Number` and puts the parse at the one assignment boundary — plus a `Finite`
newtype for the direct scalar-assignment path, where it *is* unrepresentable at no cost.

**A separate durable `u64 -> IdKind` ownership table.** The obvious reading of §10:4274's *"storage
enforces global ID ownership"*, and pi's memory backend effectively has one
(`memory.ts:689-695`). But it is one durable row per id forever — roughly 8–16 MB per million ids,
on disk and in any in-memory index — for information already present, and it becomes a second
source of truth that can disagree with the tables it describes. **Better:** derive ownership from the
per-table indexes, which exist because the indexed access paths demand them, so the check is free
and cannot drift from the data.

**Making commit observers fallible (`Fn(&Publication) -> Result<(), E>`).** pi's behaviour here is
worse than its spec admits — `#publish` has no `try`/`catch`, so a throwing listener skips every
later listener *and* makes `commit()` reject although the commit is durable and adopted, telling the
caller a successful commit failed. Making the callback fallible invites the kernel to have an opinion
about that failure, and every opinion is wrong: ignoring it hides a broken mount, propagating it
repeats pi's lie. **Better:** F1's non-fallible `Fn(&Publication, &Cx) -> ()` with no Session handle,
and the real obligation moved to where it belongs — publication across observers must be
all-or-nothing, which is a shell design requirement, not a signature.

**An automatic checkpoint heuristic, or a type that prevents checkpoint starvation.** §13:4594
explicitly declines an automatic checkpoint heuristic, and *"this caller-supplied predicate never
returns true"* is not a property any signature can express. Designing one in would be over-building
against a stated non-goal. **Better:** leave starvation a documented contract. The narrow honest
contribution is that the predicate's inputs are exactly a candidate value, the operations and
`deltas_since_base`, all available without a storage read (`spec.md:1392-1396`) — so it is a **pure
function**, a functional-core boundary, directly testable. Add observability on `deltas_since_base`
and nothing more.

**A `Validated<T>`, `Checked<Batch>` or `Verified<Storage>` generic wrapper.** Rejected on exactly
ADR-0028's grounds: such a name says only that someone once looked at it, not what is true, and it
invites being applied where no validation happened. **Better:** name the fact. `Durable`,
`Publication`, `StoredVersion`, `StoreLock` and `ReplayPlan` each state one specific thing and each
has one producer.

**A whole-Session aggregate value, to make the commit "simple".** §13:4589's first non-goal is *no
whole-Session DOM*. Atomicity is over a batch of independent records and document incarnations, not
over one tree. A session-wide state object would be a second source of truth and would make
document reclamation and per-incarnation lifetimes meaningless. **Better:** F3's keyed batch, which
is exactly the shape §13 is describing the absence of.

**Cross-process coordination beyond an exclusive lock.** §13:4596 excludes CRDT/offline multi-writer
merge, and single-writer-per-Session is an **assumption of the whole durability design** — it is also
what makes the single-committer guarantee affordable. **Better:** F6 §D refuses a second writer at
open with an error naming the holder, and never merges. Read-only reopen stays available and needs
no lock.

**Adopting an embedded database now, before the trait and the conformance suite exist.** A
sequencing rejection, not a rejection of engines; the argument is in §9.

---

## 6. Incremental migration plan

There is no migration — nothing exists yet. The *landing order* is
`docs/PICO5-PLAN.md`, twelve vertical slices with dependencies named, ordered so the riskiest
architectural commitment (F2's callback signature) is validated by a spike **first** rather than
discovered last. The order follows `RUST-DESIGN-REVIEW.md:147`'s sequence literally — centralise
parsing, introduce the types, convert boundaries, propagate, add the outcome enums, isolate the pure
transforms, and **add typestate only once the transitions are clear** — with one deviation that is
deliberate and stated in the plan: F6 §A's identity newtypes and F3's batch shape land before any
backend, because they appear in every signature and retrofitting them is the expensive direction.

---

## 7. Test implications

**Become direct parser tests, replacing scattered behaviour tests.** `DocValue::parse` absorbs the
whole of Chord's placement-rejection matrix, and **must carry a named negative test for
`f64::NAN`** — that test is the artefact that stops someone reintroducing `serde_json::to_value`,
and it is the regression this type exists to prevent. It also needs a depth-limit test, because
`Serialize` over a cyclic graph recurses without bound and `#![forbid(unsafe_code)]` does not stop a
stack overflow. `ReplayPlan::parse` absorbs the two corruption rules — missing required base, and a
version change inside a delta tail — as two `Err` arms with literal record sets and no storage, so
`materialize` is then a pure total function tested with table cases. `Lifetime::new` absorbs the
inverted-interval case while keeping the empty lifetime legal; `PageLimit::parse` absorbs zero and
over-`MAX`. `Id<K>`'s and `Seq`'s hand-written `Deserialize` impls each get a test rejecting a
string, a float, a negative and zero.

**Remain necessary as behaviour tests, unchanged.** Everything that crosses a process or a device
boundary, and no mechanism above touches any of it: whether a backend's atomicity claim is true;
whether a `Rejected` classification is honest; sequence and id monotonicity across reopen, with a
crash injected between mint and commit; JSONL recovery — torn-tail removal, unconfirmed-sidecar
removal, missing-confirmed-data failing the open; reclamation ordering and the
deferred-reclamation-on-failed-flush rule; the fsync envelope per tier; whether a poisoned Session's
batch actually committed; backend semantic equality (§10:4369 makes the **suite**, not the type, the
contract); that `adopt` is really allocation-free and callback-free (assert it by counting
allocations); and that publication across observers is all-or-nothing — which must be tested in a
**debug** build, because `[profile.release]`'s `panic = "abort"` (`Cargo.toml:447`) removes the
partial-advance hazard degenerately rather than solving it.

**Fault injection must hit three distinguishable points, not two.** Before admission;
admitted-then-unknown; and **committed-then-adoption-failed**. The third is the one a port will skip
and the one pi poisons on that nobody expects, and it is why `UncertainKind` has two variants.

**Become redundant because the invalid state no longer compiles or cannot be constructed.** *"A
draft rejects after the callback settles."* *"A `Tx` operation rejects after settlement."* *"A table
read after a table write throws."* *"Two content commands for one incarnation are rejected."* *"A
batch's write order does not affect content-before-retirement."* *"A published revision cannot be
mutated."* *"A read result does not alias the backend's index."* *"A second concurrent commit is
serialised."* Keep **one** canary each, on the module's privacy boundary, rather than a matrix —
ADR-0028 §7's rule, and the reason is the same: the canary pins the intent, the matrix pins nothing
the compiler is not already pinning.

**Compile-fail tests are warranted, for exactly four guarantees.** Each is enforced only by a
signature, and each is the kind a well-meaning later change would undo silently:

1. a `Draft` cannot be stored past the commit callback;
2. a future borrowing `Tx` cannot be `tokio::spawn`ed;
3. `tx.task(..)` does not exist on `Tx<Writing>` — **the transaction-ordering case, and yes, it is
   worth a `trybuild` case**, because it is the one guarantee
   here that replaces a named upstream error class (`ReadAfterWrite`) with nothing but an absent
   method, so its deletion is invisible in a diff;
4. a `SessionMut` cannot be obtained from `CommitOutcome::Uncertain`.

Not for the newtypes — their constructors are private and ordinary unit tests cover them.

**Do not delete the runtime checks yet.** The guarantees here are crate-scoped, not language-scoped.
`Durable` is crate-private, so a backend cannot be *made* to produce one; invariant 4 is `guarded`
and a captured effect handle still compiles; the `StoreLock` is advisory; `Id<K>`'s kind is
unverifiable from the number at a decode boundary; and a backend's failure classification can still
lie.

---
## 8. Crate layout

**Three new crates, plus one that is written only if the storage trigger fires.** The split is on
the two seams that matter: *can a backend exist without the Session* (yes, and the conformance suite
depends on it), and *does the default build link C* (no, and it must stay no).

```
crates/cyrup-pico-doc/          # the FUNCTIONAL CORE. DocValue, JsonNum, DocRoot, Path/Seg,
                                # Op/OpBatch, Revision, Tracker/Change/Prepared/Draft,
                                # apply/apply_batches/materialize, ReplayPlan::parse,
                                # choose_representation, classify_version.
                                # No tokio, no storage, no Session, no async.
                                # Deps: serde, serde_json (Number + wire form only), indexmap.

crates/cyrup-pico-store/        # records, Id<K>, Seq, DocumentPoint, Lifetime, Batch,
                                # DocumentCommand, the Storage trait, CommitError,
                                # per-scan cursors, PageLimit, MemoryStore (§11.1 reference
                                # semantics), and the conformance + reopen + fault-injection
                                # suites behind feature "conformance".
                                # Deps: cyrup-pico-doc.

crates/cyrup-pico/              # the IMPERATIVE SHELL. Session, SessionMut, Committer, Tx<S>,
                                # Draft wiring, Durable/Publication witnesses, adoption,
                                # publication, DocDef/tokens, DocumentRecord, DocAddress,
                                # Scope/Lifetime policy, migration staging, forks,
                                # CommitObserver, DocState, DocWatch, Attachment.
                                # Deps: cyrup-pico-doc, cyrup-pico-store, tokio, arc-swap.

crates/cyrup-pico-store-jsonl/  # §11.3's marker protocol over the append + deferred-fdatasync
                                # primitive cyrup-session already owns. One new dependency: fs4.

crates/cyrup-pico-store-sqlite/ # NOT written now (§9). When written: NOT in default-members, so a
                                # plain `cargo build` never compiles the C amalgamation and the
                                # Windows build stays green (docs/adr/ADR-0007-windows-scope.md).
```

Four load-bearing reasons for the split:

1. **The conformance suite must not depend on the Session.** §10's trust split is *"storage enforces
   atomicity, global id ownership, immutable conversation/entry creation, document record
   consistency and detachment; the Session owns semantic validity"* (`spec.md:4272-4277`). A suite
   that needs a Session to drive a backend cannot test that split — and the suite, not the trait, is
   what makes backends swappable (§10:4369).
2. **The pure half is large enough to earn its own crate.** `DocValue`, the operation applier, the
   replay planner, checkpoint selection and version classification are all total functions over
   explicit inputs, testable with literal values and no runtime. Keeping them behind a crate
   boundary with no `tokio` edge is what keeps them that way.
3. **`Durable` lives in `cyrup-pico`, not in the store crate.** `Storage::commit` is implemented
   *outside* the kernel, so a backend cannot be made to return an unforgeable witness.
   `Storage::commit` returns `Result<Seq, CommitError>`; the kernel's single call site converts that
   `Seq` into a crate-private `Durable`. The witness orders **kernel-internal** steps, which is
   where the mistake would be made, because the kernel is what runs adoption and publication.
4. **The SQLite crate's `default-members` exclusion is the gate, not defence in depth** — unlike
   `cyrup-test-support`, whose exclusion `Cargo.toml:55` correctly calls defence in depth. Here the
   exclusion is load-bearing: Cargo unifies features per package across one invocation, so leaving
   the crate in `default-members` is what would drag `rusqlite/bundled` into every `cargo build`.

**`cyrup-session` is not extended in place, and this is a decision rather than a preference.** Its
`SessionStore` trait (`store.rs:36-52`) is `append_line(&mut self, line: &str)` one line at a time,
plus `rewrite` and `create_exclusive` — no batch, no marker, no confirmed/unconfirmed distinction,
and a deliberately tolerant reader with no analogue of the marker cross-check §11.3's recovery
needs. What is reused is the **primitive beneath** that trait, not the trait (§9).

**`cyrup-session-svc` is not extended either.** It is the `AgentSession` facade — *"the one surface
every front-end and embedder consumes"* — and is the eventual **consumer** of this kernel, not its
home. **`cyrup-workflow-runtime` is irrelevant here**: it exists solely so a `build.rs` can snapshot
a `deno_core` extension, and has zero dependency on anything in this design.

Two naming hazards to settle before a signature is written, both carried as open questions:
`cyrup_core::EntryId` is already an `Arc<str>` 8-hex token (`cyrup-session/src/ids.rs:9-13`) while
Pico5's `EntryId` is a number in one global namespace — `cyrup-pico-store` must **not** reuse or
widen it; and `cyrup-pico-doc` must not depend on `cyrup-core` at all, precisely so that confusion
cannot compile.

---

## 9. The storage decision

### What the contract demands, restated as obligations

(1) atomicity of one admitted batch across record tables and document content; (2) a durable
strictly-increasing commit sequence, gaps allowed, surviving reopen; (3) durable allocation from one
global id namespace, never reissuing; (4) read-your-commit through the same handle; (5) detachment
of every retained write and every read result; (6) a two-class failure contract where *rejected,
nothing durable happened* is distinguishable from *uncertain*, and the latter is fatal; (7) the
indexed access paths — newest head marker at or before a cutoff, inclusive entry-id ranges paged
newest-first with every ancestry cap applied, conjunctive indexed conversation-owner filters, task
queries by five fields, exact document address resolution at current or a historical sequence,
document materialisation as newest base plus ordered delta tail; (8) historical reads over half-open
lifetimes with rewindable retention and latest-only reclamation; (9) corruption distinguished from
absence, failing the open rather than returning less; (10) opaque per-scan cursors; (11) semantic
equality across backends.

### The cost driver is (7), not (1)

Atomicity on an append-only filesystem is solved, and §11.3 is the solution: append complete
records to every affected sidecar, then append one complete main marker, and publish in memory only
after the marker write succeeds. **cyrup already owns the harder half of that primitive.**
`crates/cyrup-session/src/store.rs` does a single `write(2)` of `<json>\n` to an `O_APPEND` fd and
hands the `fdatasync` to a global coalescing worker — *"~1.5 µs instead of ~214 µs (PERF-004 §7)"* —
which is **strictly stronger than pi's shipped JSONL default** (`fsync: false`, §11.3:4420) and
strictly stronger than pi's own session manager, whose module doc records that pi *"never calls
`fsync`/`fdatasync`"*. Three things in that file transfer directly:

- `SESSION_SYNCER` (`store.rs:65-182`) — the deferred-`fdatasync` worker with burst coalescing,
  fd-identity dedup and a barrier — **is** the mechanism for `Durability::PowerLoss`;
- the fd-invalidation discipline (`self.file = None` before every rename, with its comment spelling
  out the unlinked-inode silent-data-loss failure) is literally §11.3's *"invalidates cached file
  descriptors so future appends cannot target an unlinked inode"* — already written and already
  reasoned about;
- the single-`write(2)` append and the *"kernel owns the bytes, only the flush is deferred"*
  analysis (`store.rs:18-23`) is the sidecar append primitive.

What a file backend cannot get for free is **indexed retrieval**: ancestry-capped newest-first entry
paging, conjunctive owner filters, document base-plus-tail ranges keyed by `(document, seq)`, and
task queries by five fields. A file backend holds those in memory, built at open.

**Two corrections to the pessimistic framing of that cost.** *Resident index size is not the
problem* — the indexes are id→offset maps plus small key tuples, tens of bytes per record, so 100 k
entries is a few MB and RAM is not the bound. *Open time is the problem, and it is bounded by the
main log, not by document content* — `main.jsonl` holds table writes, document records and one
marker per commit; sidecars hold content. §10:4345 forbids an open-time all-document scan, and a
file backend can honour that literally, because the sidecars are never read at open. With F6 §D's
marker-carried offsets it is honoured in spirit too. So **open is O(commits + table records)** — and
since §13:4598 declines main-log compaction, that number only grows.

**That is a stated envelope, not a behavioural concession.** §11's own framing is that backends
differ only in *"durability envelope and performance, and those differences are stated, not
emergent"*. A file backend whose open cost grows with history is in the same category as §11.3's two
fsync tiers, and pi ships JSONL as a peer backend.

### Options, with the cost each actually carries

**(a) Files only, JSONL-shaped, in-memory indexes built at open.** Zero new database dependencies
(one small one: `fs4`). Reuses a primitive cyrup already has, already fsyncs and has already reasoned
about. Windows-clean. Inspectable with `jq`, which matters for support. **Cost:** open is
O(commits + table records) and unbounded over a session's life; §10's query semantics are
implemented by hand and are where the bugs will be; no compaction, ever.

**(b) SQLite (`rusqlite`, bundled).** Matches §11.2 one-to-one, so atomicity, the sequence row, the
indexes and the base-plus-tail ranges are the engine's job, and pi's reference schema is a known-good
shape to conform against rather than a design to invent. Bounded open regardless of history. Gives
cross-process locking for free, which F6 §D otherwise buys with `fs4`. **Cost:** cyrup's first
embedded database and first C dependency; the amalgamation adds roughly 20–40 s to a cold build,
which is why the crate must be outside `default-members`; schema migrations become a permanent
obligation; `#![forbid(unsafe_code)]` holds for cyrup's crates but not for what they link.

**(c) `redb`.** Pure Rust, no C, no build-time hit, transactional, ordered keys, single-writer MVCC.
(1)–(4) and the ordered-range half of (7) come free. **Cost:** every secondary index for the
conjunctive owner filters and the five-field task query is hand-built as another table — not much
worse than SQLite, where the `CREATE INDEX` statements are also hand-written, except that the index
*selection* is hand-written too. Smaller ecosystem, and an on-disk format whose migration story
cyrup alone owns.

**(d) Contract-first: land the trait, ship memory + files, add an engine behind the same trait on
measurement.**

### Recommendation

**(d), sequenced, with (b) named as the engine and a budget written into this ADR.**

This is not deferral. Three things are **irreversible** and must be right before any backend exists,
because every type-level guarantee in this document is built out of them: the **two-class failure
outcome** (F1 — it shapes every caller's error handling), the **keyed batch** (F3 — it changes every
backend's apply loop and deletes validation in three places), and the **`&mut self` single-committer
signature** (F1/F3 — it is what makes the one mutation line a compiler-visible fact). Which engine
sits behind them is comparatively cheap to change later, and §10's own structure says so: `interface
Storage` exists precisely so backends are swappable, and §10:4369 makes the conformance suite the
contract.

Memory and files are **not placeholders**. Memory is §11.1's normative reference semantics and the
substrate for the whole conformance suite — and note that in cyrup it is *simpler* than pi's, because
`memory.ts`'s recursive clone has no Rust equivalent (F4). Files are what cyrup already does, what
keeps a session inspectable, and what pi ships as a peer.

**SQLite rather than redb**, when the trigger fires. redb's advantages are real, but the reason to
adopt an engine at all is to stop hand-building indexed retrieval — and redb gives the ordered ranges
while leaving the index selection hand-built, which is the half that was hurting. SQLite also comes
with a specified reference schema (§11.2) and pi's own behaviour to cross-check against, which is
worth a great deal for obligation (11).

**The trigger is a number, and it is stated here rather than left to taste:**

> Write `cyrup-pico-store-sqlite` when a p95 real cyrup session's store exceeds **either** 200 ms to
> open **or** 64 MB resident index.

That threshold is a **placeholder with no measurement behind it** (§14, open question 2). It is
stated as a number anyway, because a trigger without one is not a trigger.

---

## 10. Core types and signatures

Collected for an implementer. Everything here appears in a finding above with its reasoning; this is
the index.

```rust
// ---- identity (F6 §A) --------------------------------------------------------
pub trait IdKind: private::Sealed + 'static { const KIND: IdKindTag; }
pub struct Conversation; pub struct Entry; pub struct Task;
pub struct Submission;   pub struct Document;

#[repr(transparent)] pub struct Id<K: IdKind>(NonZeroU64, PhantomData<fn() -> K>);
pub type ConversationId = Id<Conversation>;
pub type EntryId        = Id<Entry>;          // NOT cyrup_core::EntryId — see §8
pub type TaskId         = Id<Task>;
pub type SubmissionId   = Id<Submission>;
pub type DocumentId     = Id<Document>;

#[repr(transparent)] pub struct Seq(NonZeroU64);
pub enum DocumentPoint { At(Seq), Current }
pub struct Lifetime { created_at: Seq, retired_at: Option<Seq> }   // private; contains() only
pub struct DefVersion(NonZeroU32);

// ---- the pure document core (F4, F6 §C) -------------------------------------
pub enum DocValue { Null, Bool(bool), Num(JsonNum), Str(Arc<str>),
                    List(Arc<Vec<DocValue>>), Map(Arc<DocMap>) }
pub struct DocRoot(Arc<DocMap>);
impl DocValue { pub fn parse<T: Serialize + ?Sized>(v: &T) -> Result<DocValue, NotStrictJson>; }

pub enum StoredContent { Base { version: DefVersion, value: DocRoot },
                         Delta { version: DefVersion, ops: OpBatch } }
pub struct ReplayPlan { /* private */ }
impl ReplayPlan { pub fn parse(r: &[StoredContent]) -> Result<ReplayPlan, Corruption>; }
pub fn materialize(plan: &ReplayPlan) -> DocRoot;                 // pure, total
pub struct StoredVersion { version: DefVersion, _seal: () }       // witness; no serde at all

// ---- the storage contract (F1, F3, F6 §B/§C) --------------------------------
pub struct Batch { /* five BTreeMap fields, private */ }
pub enum DocumentCommand { Create {..}, Copy {..}, Change {..}, RetireOnly }
pub enum Retire { Keep, Retire }
pub enum CommitError { Rejected(RejectedReason), Uncertain(Box<dyn Error + Send + Sync>) }
pub enum StorageFailure { Corrupt(Corruption), Io(io::Error), HistoryNotRetained { document: DocumentId } }
pub struct CursorBytes { store: StoreId, payload: Box<[u8]> }     // no Deserialize
pub struct PageLimit(NonZeroU32);
pub struct Page<T, C> { pub items: Vec<T>, pub next: Option<C> }

#[async_trait] pub trait Storage: Send {
    async fn commit(&mut self, batch: Batch, cx: &Cx) -> Result<Seq, CommitError>;
    async fn mint<K: IdKind>(&mut self, cx: &Cx) -> Result<Id<K>, CommitError>;
    async fn document(&self, id: DocumentId, at: DocumentPoint, cx: &Cx)
        -> Result<Option<StoredDocument>, StorageFailure>;
    async fn find_document(&self, addr: &DocumentAddress, at: DocumentPoint, cx: &Cx)
        -> Result<Option<DocumentRecord>, StorageFailure>;
    async fn find_latest_head_marker(&self, c: ConversationId, upto: EntryId, cx: &Cx)
        -> Result<Option<EntryId>, StorageFailure>;
    async fn scan_entries(&self, q: &EntryQuery, limit: PageLimit, from: Option<EntryCursor>, cx: &Cx)
        -> Result<Page<EntryRecord, EntryCursor>, StorageFailure>;
    // … conversations, tasks, submissions, documents: same shape, each with its own cursor type
}

// ---- the kernel (F1, F2, F5) ------------------------------------------------
#[derive(Clone)] pub struct Session(Arc<SessionShared>);
pub struct SessionMut { /* private; not Clone, never in an Arc */ }
#[derive(Clone)] pub struct Committer(mpsc::Sender<Job>);
pub struct Reading(()); pub struct Writing(());
pub struct Tx<'tx, S> { /* invariant 'tx, !Send, !Clone, no Session, no effects */ }
pub struct Draft<'d, T> { /* borrows the open change */ }
pub(crate) struct Durable { seq: Seq, _seal: () }
pub(crate) struct Publication { seq: Seq, changes: Arc<[Change]>, _seal: () }
pub enum CommitOutcome<R> { NothingToCommit{..}, Committed{..}, RolledBack{..}, Uncertain(..) }
pub enum CommitReply<R>   { NothingToCommit(R), Committed{..}, RolledBack(..), SessionDead(..) }
pub trait CommitObserver: Send + Sync + 'static { fn observe(&self, p: &Publication<'_>); }

// ---- the file backend (F6 §D) -----------------------------------------------
pub struct StoreLock { /* RAII; not Clone */ }
pub enum Durability { ProcessCrash, PowerLoss }
```

**The serde audit, consolidated**, because `RUST-DESIGN-REVIEW.md:73` makes it mandatory and this
subject persists and re-reads every record on every open:

| type | `Serialize` | `Deserialize` | why |
|---|---|---|---|
| `Id<K>`, `Seq`, `DefVersion` | yes | **hand-written** | a derived impl over a private `NonZeroU64` still accepts any nonzero number. Require a u64 (reject string, float, negative), reject 0, build through the private constructor. The **kind** cannot be checked from the number — it is type-directed by the decode site — so the backend must still assert number→kind ownership against its committed index. |
| `Seq` additionally | — | gated | a decoded `Seq` that does not strictly increase is **corruption** and must fail the open, not be normalised (pi: `memory.ts:253`, `jsonl/storage.ts:543-546`). |
| `DocValue` | yes | yes (derive-equivalent) | safe: JSON has no `NaN` literal and `serde_json::Number`'s own `Deserialize` cannot yield a non-finite, so the decode path cannot forge what `parse` rejects. **Still needs the same depth limit**, because a damaged file can nest arbitrarily — that is corruption, not absence. |
| `DocRoot` | yes | hand-written | require a JSON object, so the error names the document. |
| `Batch` | no | **never** | an in-process assembly product. A derived `Deserialize` would reintroduce every illegal state through a map with duplicate keys silently last-wins, which is the quiet version of F3's bug. |
| `DocumentCommand`, `DocumentBase`, `DocumentContent` | yes | yes, **audited with `StoredVersion`** | a recovered `Delta { continues }` whose version does not match the base it follows is corruption (§10:4358) and must fail the open rather than be normalised. |
| `StoredVersion` | **no** | **no** | an in-process witness. A `Deserialize` would let a recovered record mint a version claim. |
| `Durable`, `Publication` | **no** | **no** | in-process proofs about one commit. The `_seal: ()` field makes a struct literal outside the module a compile error; `derive(Deserialize)` would reopen exactly that path. |
| `Tx`, `Draft`, `DocHandle`, `Reading`, `Writing` | no | **never** | a `Deserialize` for `Tx` would construct a transaction with no storage behind it; for `DocHandle`, a forged slot index. |
| `Session`, `SessionMut`, `Committer` | no | **never** | a `Deserialize` for `SessionMut` fabricates a second mutation handle, which is the one thing F1 exists to forbid. |
| `RejectedReason`, `Corruption`, `UncertainCommit` | **yes, only** | **no** | they must be loggable and surfaceable in a diagnostic without becoming a construction path. A deserialisable `RejectedReason` would let a corrupted log line or an IPC boundary reintroduce the misclassification the closed enum forbids. |
| `CursorBytes` | withheld | **no** | a derived `Deserialize` would let any host bytes become a cursor for any scan, undoing both the per-scan typing and the `StoreId` check. If a host requirement for persistent pagination appears, `Deserialize` is hand-written to check `StoreId` and the scan tag, plus pi's shape check. |
| `PageLimit` | yes | hand-written | through `parse`; a derived impl over a private `NonZeroU32` accepts values past `MAX`. |
| every persisted record type | yes | yes | **and no `#[serde(borrow)]`, no `Cow<'_, _>` anywhere in them.** Every decoded value is `'static`-owned. That rule is what makes F4's `Arc` return type sound across a reopen or a cache eviction. |

---

## 11. How this meets ADR-0002 at the extension boundary

`docs/adr/ADR-0002-extension-io-is-serde.md` holds that every value crossing the extension boundary
crosses **as a value**, on the WASM and native tiers alike, and that the encoding never licenses
dropping a serde-representable field. This kernel is on the right side of that by construction, and
in one place it strengthens it:

- **A document value is already a value.** `DocValue` is strict JSON with no interior mutability and
  no host pointers. An extension that owns a document definition sees `DocRoot::parse::<T>(&t)` on
  the way in and a typed projection on the way out; nothing crosses as a handle. This is a stricter
  statement than ADR-0002 requires, because the *stored* form and the *crossing* form are the same
  type.
- **A draft does not cross the boundary, and must not.** `Draft<'d, T>` is a borrow of an in-process
  transaction: non-`Send`, non-`Clone`, non-`'static`. It cannot be marshalled, which means an
  extension cannot be handed one and §1 invariant 6 cannot be violated from the guest side at all.
  An extension that wants to mutate a document returns **data** describing the mutation, and the host
  applies it inside the transaction — which is ADR-0002's rule arrived at from the opposite
  direction.
- **A publication crosses as data.** `Publication` is crate-private and consumed by the kernel's one
  publish; what reaches an extension is a serialisable frame. §13:4592's non-goal (no independently
  maintained event state) means that frame is derived from the committed records, never a second
  journal.
- **The reserved-namespace parse is where ADR-0002's "never drops a field" rule bites.** A `Kind`
  newtype parsed at registration rejects `pi.`-prefixed kinds for third-party definitions; the parse
  must reject rather than sanitise, because a silently-rewritten kind is a dropped field in
  ADR-0002's sense and, worse, is permanent — a value migration cannot rename a document kind
  (§12:4523-4527).
- **One `CYRUP-DELTA` is owed at this boundary**, and per ADR-0002 rule 7 as harmonised by ADR-0008
  §A.3 it carries both halves — the tagged two-sided upstream citation **and** this ADR's path: the
  draft surface is path-addressed method calls rather than property assignment
  (`tmp/pi` @ `v1.0.0` `packages/chord/src/delta/README.md`; `docs/adr/ADR-0030-durable-rust-architecture.md` §4).

---

## 12. How ADR-0028's decision rule applies, and where this lands differently

ADR-0028 is the precedent, and its rule is applied here unchanged. It reaches the opposite verdict
for its own subject on **one clause**, and that clause is the whole difference:

> the ACP SDK's `Builder`/`ChainedHandler` registers every handler for the life of the connection —
> **there is no ownership path along which a compiler could withhold a method** (ADR-0028:42)

In §4 there are three such paths, and they are the only reason typestate is justified here: the
transaction is handed to a closure and taken back; a draft borrows the transaction; the mutation
handle is moved into the commit and returned only on a non-fatal outcome. Every other rejection
test at `RUST-DESIGN-REVIEW.md:55-61` passes for all three — not persisted, not resumed, not stored
heterogeneously, not selected by external events, and the compiler-visible lifecycle **ends when the
closure returns**.

Where ADR-0028's reasoning transfers **verbatim**, it is applied rather than re-derived:

- **The `ToolCallStream` rejection** (ADR-0028:659 — *"the streams live in a `HashMap<ToolCallId, _>`
  and are selected by a value that arrives off the wire, so the map needs one concrete value
  type"*) is the exact mechanical reason the task machine, the document lifecycle and the backend's
  health are enums and flags rather than typestate. Documents live in a `DocumentId`-keyed map and
  are selected by storage; tasks live in a `TaskId`-keyed table and are selected by a scheduler.
- **The session-lifecycle rejection** (*"a `Session<Prompting>` type would encode about four seconds
  of a lifecycle measured in days"*) is why `Session<Open|Closing|Closed>` is rejected in §5, with
  §12:4575-4577's non-cooperative-code footgun as the concrete bound.
- **The `Validated<T>` rejection on naming grounds** is applied in §5 and is why every witness here
  is named for the fact it states.
- **ADR-0028's `HandlerOutcome` / "only the shell holds `cx`" split** is the same move as F1's
  actor: the shell owns the channel and the I/O, and the decisions are in the pure part.
- **ADR-0028 §7's canary rule** — *"keep one test of each as a canary on the module's privacy
  boundary rather than a matrix of them"* — is adopted verbatim in §7.

Where this document lands differently, the clause is named: **typestate in three narrow consuming
forms**, because the ownership path ADR-0028 could not find exists here. The scope is small on
purpose — two states on `Tx`, one consuming move on `SessionMut`, one borrow for `Draft` — and three
of the five findings (F1's enums, F3's keyed batch, F4's value type) are **not typestate at all**
despite being the highest-value results.

---

## 13. Rejected architectural alternatives

**Port `packages/durable`'s TypeScript shape into Rust.** A faithful transliteration of a
service/class architecture would be the wrong artefact even if it passed every test, and §4's whole
storage-ownership section plus `memory.ts`'s recursive clone would be translated instead of deleted.
ADR-0029 decides this; it is recorded here because it is the alternative an implementer will be
tempted by when a conformance assertion fails and pi's code is right there.

**One crate instead of three.** Rejected on the conformance-suite seam: a suite that needs a Session
to drive a backend cannot test §10's trust split, and the suite is what makes backends swappable.
The pure/shell boundary is the second reason, and it is the one that keeps the document core testable
with no runtime.

**Extend `cyrup-session` in place.** Rejected in §8 on the trait's shape — one line at a time, no
batch, no marker, no confirmed/unconfirmed distinction — and on the id vocabulary (string ids versus
one global numeric namespace). The *primitive* beneath it is reused; the trait is not.

**A shared `Arc<Session>` with an internal commit mutex, as pi has.** Gives up both the consuming
form of invariant 8 and the `&mut self` storage signature that makes §10:4321's assumption a checked
fact, and invites the `try_commit`-plus-separate-read-lock refactor whose concrete failure F1
describes. Rejected; the `Committer` ticket exists precisely so nobody needs to reach for
`Arc<Mutex<SessionMut>>`, and `SessionMut` being non-`Clone` and non-`Sync` is what makes that
workaround visibly wrong rather than merely unwise.

**Designing in anything §13 excludes.** Eleven non-goals, each a temptation with a name: a
whole-Session DOM, a volatile/optimistic progress channel, a kernel event journal, session-scoped
rewindable documents, an automatic checkpoint heuristic, automatic third-party view mounting, CRDT
merge, SQL translation of Chord operations, JSONL compaction, automatic corruption repair, a
compatibility layer for removed Pico prototypes, and forced termination of non-cooperative extension
code. Building any of them is **over-building, not ambition**. §5 argues the four that look most
like features.

---

## 14. Open questions

1. **Windows durable rename.** `std::fs::rename` maps to `MoveFileEx` with `MOVEFILE_REPLACE_EXISTING`
   but **not** `MOVEFILE_WRITE_THROUGH`, and there is no directory-fsync equivalent. §11.3's
   reclamation path depends on the rename being durable before the authorising marker is trusted.
   Does cyrup take a `windows-sys` dependency in the JSONL backend to call `MoveFileEx` directly,
   accept NTFS metadata journalling as sufficient, or state the Windows reclamation envelope as
   weaker? `docs/adr/ADR-0007-windows-scope.md` puts Windows in scope, so this needs an answer rather
   than a `cfg`-gated TODO.
2. **The SQLite trigger budget needs a measurement.** §9 states 200 ms open / 64 MB resident index
   at p95 as a placeholder. Nobody has measured real `~/.cyrup` session sizes — total commit count
   and table-record count are the two numbers that matter, because open is
   O(commits + table records). Measure before the threshold is treated as decided.
3. **Does the marker-carries-sidecar-offsets design survive a second look?** It depends on the
   exclusive lock making byte offsets knowable for an `O_APPEND` fd, which is sound for a single
   appender but means the offsets are only as trustworthy as the lock. Recovery must validate every
   offset against actual sidecar length and treat a past-EOF offset as corruption. Is that validation
   cheap enough to keep the open-time saving it exists to buy?
4. **Is `for<'tx> AsyncFnOnce(Tx<'tx, Reading>) -> Result<(R, Tx<'tx, Writing>), _>` ergonomic in
   practice?** This is the sharpest edge in the design and it has not been written against a real
   borrow checker. Slice S0 of `docs/PICO5-PLAN.md` exists to answer it before anything is built on
   it. The fallback loses the consuming transition and is recommended against.
5. **Does the harness ever legitimately need a commit callback to await something that is not a
   storage read?** If the answer is a clean no, invariant 4's residual shrinks a great deal and the
   debug line-hold budget can be tight. If some hook path genuinely needs to await an in-memory
   registry, identify it now — because it is the path that will later be used to justify awaiting a
   model.
6. **cyrup has no value-carrying `Context`.** The tree has 65 `CancellationToken` uses and nothing
   that carries values, while §9.2 requires a delivered frame to carry the commit's context *values*
   without inheriting its cancellation. The `FrameContext` design — a type with no token field, so
   inheriting cancellation is unrepresentable — is provisional on what a cyrup value-context turns
   out to be.
7. **Chord's string-trim operation counts UTF-16 code units.** `["t", path, count]` is a
   persisted-format decision, and Rust strings are UTF-8. Whether cyrup stores UTF-16 units (wire
   compatible, awkward), byte offsets (natural, incompatible) or char counts is unsettled, and it is
   settled *once* because it is persisted. Only matters if cyrup ever exchanges operation batches
   with a chord peer; ADR-0029's scope says it does not, so byte offsets are the provisional answer
   with the incompatibility recorded.
8. **The `EntryId` collision.** `cyrup_core::EntryId` is an `Arc<str>` 8-hex token; Pico5's is a
   number in one global namespace. Reusing or widening the existing type would let session-level
   string ids flow where a mintable numeric id belongs. Decide the name before the first signature:
   a distinct `cyrup_pico_store::EntryId`, or a rename on one side.
9. **Do cyrup's existing file sessions migrate into a Pico5 store, or coexist?** §13:4599 declines a
   compatibility layer for pi's own removed prototypes, so cyrup owes nothing *upstream* — but
   cyrup's existing sessions are real user data. Migrating means synthesising ids and sequences for
   records that have neither; not migrating means two session formats in the tree. A product
   decision, flagged rather than resolved.

---

## How to reverse this

**The sentence.** *"Use `serde_json::Value` and runtime checks; skip the typestate."*

**What would have to change in the tree.** F2's two marker types and the `for<'tx>` bound come out
and are replaced by a monotone `has_table_write` flag plus a `ReadAfterWrite` error, a `#sealed`
equivalent on `Tx`, and a per-access assertion on `Draft` — that is, pi's three mechanisms,
reimplemented. F1's `CommitOutcome` collapses to `Result<_, StorageError>` plus a poison flag on a
shared `Arc<Session>`, and `Storage::commit` becomes `&self`. F4's `DocValue` becomes
`serde_json::Value`, which forces a deep copy per revision — so the throttled live-document path
needs a different design — and reintroduces `memory.ts`'s recursive clone in every backend, plus a
strict-JSON walk of prepared operations and selected bases that `spec.md:1348-1351` explicitly
optimises away.

**The cost of the option that was rejected.** Roughly: four runtime mechanisms reimplemented, three
copies of eight batch validations ported instead of deleted, a deep copy on every document read and
write, one specification section's worth of ownership discipline carried as prose, and a `NaN`
silently persisting as `null` until the next restart.

**What would change this decision legitimately, as opposed to reversing it.** Open question 4
returning *no* — if `for<'tx> AsyncFnOnce` with a returned `Tx` proves unworkable against a real
borrow checker, F2's transition falls back to a flag and invariant 4's and the read-ordering
guarantees drop from `typestate` to `checked`. That is a measured retreat, not a reversal, and slice
S0 exists to find out before anything depends on it. Open question 2's measurement firing moves the
storage engine and changes nothing else — §9's whole point is that the engine is the reversible
decision and the trait's shape is not.
