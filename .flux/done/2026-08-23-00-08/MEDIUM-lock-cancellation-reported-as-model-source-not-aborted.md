---
title: Lock Cancellation Reported As Model Source Not Aborted
priority: MEDIUM
stage: qa
status: completed
updated: 2026-08-23 09:10
---

# QA: 8/10 — the mapping is right; its own doc block points at nothing

The change is **correct and accepted**. `store_err`
([`models_store.rs:208-214`](../../crates/cyrup-config/src/models_store.rs)) is a two-arm match,
byte-identical to the prescribed text, and an independent adversarial review found **no blocking
defects**: no non-cancellation error can now be reported as `Aborted`, no consumer behaves worse,
and the spec's non-reachability claim was independently re-verified.

Do not re-implement it. What follows is only what is outstanding.

---

## 1. REQUIRED — both citations in the new doc block are dead

`models_store.rs:201-202`. The block argues entirely from citation, and both pointers miss by
exactly six lines:

| The doc says | Actually at | What is at the cited line today |
| --- | --- | --- |
| `lock.rs:99-102` — layer-1 `Cancelled` | **`lock.rs:108`** | the `acquire` signature and its `cancel` doc paragraph |
| `lock.rs:126` — layer-2 `Cancelled` | **`lock.rs:132`** | `while !held {` |

**Cause, and the lesson.** `crates/cyrup-config/src/lock.rs` was modified *after* this file — the
HIGH task's doc rework appended six lines above both sites. The spec prescribed the pre-shift
numbers and this task reproduced them faithfully, so the implementation is not at fault; the
citations went stale between the two execs. This is precisely the defect class the branch exists to
remove, now self-inflicted: a comment pointing at code that is not there.

**Fix:** in the `store_err` doc block, change `lock.rs:99-102` → `lock.rs:108` and `lock.rs:126` →
`lock.rs:132`. Verify by `sed -n '108p;132p' crates/cyrup-config/src/lock.rs` — both must show a
`ConfigError::Cancelled` construction. Nothing else in the block changes.

**Do this last, after every other queued task that touches `lock.rs` has landed** — otherwise it
goes stale again. `LOW-spawn-blocking-join-error-reported-as-lock-contention` and
`MEDIUM-filelock-drop-order-comment-states-a-false-rust-rule` both still edit that file. Better
still, consider whether these two pointers need line numbers at all: "over the single `Err` arm of
`KeyedLocks::guard`" and "the layer-2 retry loop" already name their targets unambiguously, and a
nameless reference cannot rot. Prefer dropping the numbers to re-deriving them.

## 2. REQUIRED — the module-doc sentence covers only one of the two layers

`models_store.rs:26` says a cancellation is one "whose `signal` fired — before the lock, or while
the acquire was queued behind another **task**". Layer 1 queues behind another task; layer 2 — the
common source, and the reason this fix was urgent — queues behind another **process**. The
sentence's whole job is to cover both.

**Fix:** make it "queued behind another task or process". One word.

## 3. REQUIRED — Definition-of-done item 2 promises more than the code delivers

It reads: *"For each of `read`, `write` and `delete`, a `signal` cancelled while the acquire is
queued yields `Err(ProviderError::Aborted)`."* Not unconditionally true for `write`/`delete`.
`lock.rs:94-97` documents that a token cancelled **while a `try_lock` attempt is already in flight**
still yields a granted lock, and `write`/`delete` deliberately do not re-check after acquisition —
pi's placement, and intentional. In that window they return `Ok(())` and the write lands. Only
`read` is fully covered, by the second `throw_if_aborted` at `:311`.

**Fix:** reword item 2 to scope the guarantee to a cancel observed *before* an attempt is in flight,
and state the acquire-win race as the known, intended exception. Do not change any code for this.

---

## Record only — no action in this task

Verified, judged, and deliberately excluded. Listed so they are not rediscovered as new.

- **`ConfigError::Core(CoreError::Cancelled)` falls into the `other =>` catch-all**
  (`models_store.rs:212`). `CoreError::Cancelled` exists (`cyrup-core/src/error.rs:8`) and its own
  doc says it "Maps to a terminal `aborted`", and `ConfigError::Core` carries `#[from]` — so a
  future `?` on a `CoreError` inside `FileLock::acquire` or `write_atomic` would regress this fix
  with **zero compiler signal**. Unreachable today: `ConfigError::Core` has **no construction site
  anywhere in the workspace** (grep returns only the enum definition). An explicit second arm would
  make the guarantee structural rather than incidental; the spec scoped it out and that still holds.
- **The abort identity is re-flattened one layer up.** `collection.rs:517-519` and
  `images/mod.rs:475-477` re-wrap any non-`ModelSource` provider error into
  `ProviderError::ModelSource(format!("Model refresh failed for {id}: {e}"))`. So even if a store
  cancellation propagated that far, `is_aborted()` would be false again at that caller. Pre-existing
  and not a regression, but it **caps this fix's end-to-end reach** — nobody should assume
  cancellation survives to the stream boundary through that path.
- **`ProviderError::Aborted` is a unit variant**, so the store's identity is erased from the
  message: `"aborted"` rather than `"model source refresh failed: cancelled"`. No information is
  genuinely lost (`ConfigError::Cancelled` is itself a unit variant displaying `"cancelled"`), and
  it matches what `throw_if_aborted` already produces.
- **The new arm has no test.** A mutation swapping `ProviderError::Aborted` for
  `ProviderError::ModelSource(Box::new(other))` would leave all 222 tests green. Tests are another
  team's scope on this project; the spec defers it explicitly.
- **The `.flux/SCOPE.md` ordering edge was never recorded.** The spec's Ordering section asked for
  it. Now moot — both tasks have landed, `lock.rs` after `models_store.rs`, so the constraint is
  spent. Recording a discharged edge adds nothing; skip it.
- **Module doc item 4 reads as an erratum.** `:21-23` still literally says a write/delete that
  "could not take the cross-process lock … surface[s] as `ProviderError::ModelSource`", walked back
  by the appended sentence at `:25`. Spec-faithful — it prescribed "no other edit to the block" —
  and the paragraph as a whole is now correct.

## Already accepted — do not redo

- **Mapping completeness.** All 12 `ConfigError` variants enumerated
  (`crates/cyrup-config/src/error.rs:9-59`). `Cancelled` is genuinely the only diverter: it has no
  `#[from]` and exactly two construction sites workspace-wide (`lock.rs:108`, `:132`), both genuine
  token-fired cancellation. `Io`/`Serde`/`Lock` → `ModelSource` is right; the other eight are not
  constructible on any path reaching `store_err`.
- **Every reachable path.** All six `store_err` call sites checked. `read_latest` propagates
  `ConfigError` unmodified (identity `From`, no `map_err`); `write_all` → `write_atomic` +
  `ensure_dir` yield only `io_err`; the hand-built `Serde` lands on `other`. No non-cancellation
  error can reach `Aborted`.
- **Downstream semantics.** `code()` → `"aborted"`; `is_aborted()` is `matches!(self, Aborted)`;
  `into_error_message` selects `StopReason::Aborted`; `reproduce()` round-trips; `pi_messages.rs:528`
  forwards the wire flag and suppresses diagnostic attachment. No consumer behaves worse — every
  `RemoteCatalog` store call discards the error, and nothing retries, caches, logs or counts on it.
- **`Box::new(other)`** is identical to the old `Box::new(e)` — no behavioural or allocation change.
- **Intra-doc link** `[`ProviderError::Aborted`]` resolves via the `use` at `:32`, same as the
  pre-existing `ModelSource` link at `:23`. No new `use`.
- **Spec fidelity.** Both hunks byte-identical to the prescribed text. Zero deviation.
- **Non-reachability re-verified independently.** All four `Some(signal)` constructions are inside
  `#[cfg(test)]`; every production caller passes `None`; the only production `FileModelsStore` is
  built at `crates/cyrup/src/provider.rs:76` and handed to `RemoteCatalog`, which passes `None` at
  all six sites. Latent contract fix, not a live bug — as the spec said.
- **DoD 1, 3, 5, 6, 7** all genuinely met, including exactly-one-file (verified by mtime, not git).
