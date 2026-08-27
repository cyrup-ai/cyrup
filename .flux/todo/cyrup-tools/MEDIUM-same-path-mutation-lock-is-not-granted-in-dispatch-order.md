---
title: Same-path mutation lock is not granted in dispatch order
priority: MEDIUM
tool: write
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: new
status: done
updated: 2026-08-27
---

# Same-path mutation lock is not granted in dispatch order

## What pi does

`withFileMutationQueue` funnels every registration through one module-scope chain: `registrationQueue = registration.then(...)` (file-mutation-queue.ts:5, :33, :46-49). The realpath key computation (`getMutationQueueKey`, :16-26) therefore runs serialized in call order, and each caller links its promise onto `fileMutationQueues.get(key)` in that same order (:35-42). Two mutations dispatched for the same file in order A then B are guaranteed to run A then B, so the surviving file content is deterministically B's.

## What cyrup-tools does

/home/user/cyrup/crates/cyrup-tools/src/lock.rs:175-186: `guard()` first awaits `Self::key(path)` — an async `tokio::fs::canonicalize` (lock.rs:153-161) run independently per call, with no serializing registration chain — and only then enters `KeyedLocks::guard`, whose waiter order is the order tasks reach the tokio mutex (/home/user/cyrup/crates/cyrup-core/src/keyed_lock.rs:74-80 + the per-key `tokio::sync::Mutex`). There is no equivalent of pi's `registrationQueue`; ripgrep over /home/user/cyrup/crates/cyrup-tools/src finds no other ordering mechanism.

## User-visible impact

When the model emits two parallel write (or write+edit) calls for the same path in one batch, pi always applies them in the order they were issued; cyrup can apply them in the reverse order if the two `canonicalize` calls resolve out of order, so the file's final contents can be the earlier call's payload rather than the later one's.

## Parity action

Establish the queue position before the async key resolution — e.g. serialize key computation/registration through a single global chain (a process-wide registration mutex around `Self::key` + entry insertion) so lock acquisition order equals call order, matching file-mutation-queue.ts:5/:33/:46-49.

## Why this gap is real

An adversary agent was tasked with **refuting** this finding by locating the capability in the Rust under another name. It could not:

> Confirmed absent after searching for the behaviour, its synonyms and its primitives. Pi's ordering guarantee is real and comes from a module-scope registration chain: file-mutation-queue.ts:5,33,46-49 (and the harness twin at packages/agent/src/harness/tools/file-mutation-queue.ts:6,31,43-46) funnels every call's async realpath through `registrationQueue`, and the queue link `fileMutationQueues.set(key, chainedQueue)` therefore happens in call order. That call order is dispatch order because `withFileMutationQueue` is invoked synchronously from execute (write.ts:210, edit.ts:336), reached synchronously from `executePreparedToolCall` (agent-loop.ts:678), which the parallel batch invokes in source order (agent-loop.ts:540-542). Cyrup has no analogue: FileMutationLocks::guard (lock.rs:175-186) awaits a per-call `tokio::fs::canonicalize` (lock.rs:153-161) with nothing serializing it, then enters KeyedLocks::guard (keyed_lock.rs:145-168) where the tokio Mutex is FIFO only from the moment each task arrives; the batch spawns every prepared call onto a JoinSet (agent/run/tools/exec.rs:96-107), and write/edit take that parallel path since neither overrides execution_mode (write.rs:68, edit.rs:141). Ripgrep over crates/cyrup-tools/src and crates/cyrup-core/src for registration/fifo/ticket/sequence/ordered/Semaphore/Notify/Barrier finds no ordering mechanism, and `registrationQueue` appears nowhere in the repo (code or docs), so this is not a renamed or relocated port. Severity corrected down from a corruption-class reading: mutual exclusion itself IS implemented (process-global map, one mutex per realpath key), so no interleaved/torn file can result — only the tie-break among mutations dispatched simultaneously for one path differs, and only when the model emits a self-contradictory same-path batch. It is still medium rather than low because when it does fire nothing is reported wrong: both calls return success while the file can retain the earlier call's payload.

## Definition of done

1. The capability described under *Parity action* is implemented in `crates/cyrup-tools`.
2. A test pins the new behaviour against the pi semantics quoted above.
3. `cargo check --workspace --all-targets` and `cargo clippy` stay clean.
4. Behaviour that pi does NOT have is not introduced — this is a parity task, not a redesign.
