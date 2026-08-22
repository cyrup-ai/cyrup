---
stage: qa
status: completed
updated: 2026-08-22 18:06
---

# Make Finalizing's Final-Value Handling Match Its Documented Contract

## Description

Two independently verified defects in crates/cyrup-core/src/event_stream.rs converge on the same symptom — `result()` does not deliver the final value the docs promise. (a) The trait doc at :23-25/:27 and the note at :127-129 say `result()` resolves "when the terminal item fires" and "never blocks", but `Finalizing::result` (:105-118) ignores `is_complete` entirely and drains the receiver to `None`; since `push` (:51-67) sets `done` but leaves `self.tx` intact, a caller that pushes a terminal and holds an un-ended sink hangs forever (reproduced: a 300ms `tokio::time::timeout` around `result()` after a terminal push times out). (b) `push` invokes the caller-supplied `extract` closure *inside* the `final_value` mutex guard (:58-60), so a panicking closure poisons the mutex; afterwards `end`'s let-chain (:72-76) silently drops an explicit `end(Some(v))` and `result()` (:116-117) falls through `.lock().ok()` to the terminal-less fallback with no error and no log (reproduced: `end(Some(99))` yielded i32::MIN). Fix both, and pin both with tests — the two existing tests (:154-172) all call `end()` first, so neither path is currently covered.

## Evidence

```
crates/cyrup-core/src/event_stream.rs:23-27 and :127-129 (docs) vs :105-118 (result drains rx to None) and :56-61 (extract called under the lock) / :70-79 (only `end` clears tx). Probes (run, then reverted; `git status --porcelain` clean): "PROBE: HUNG after terminal item (doc says it should resolve)"; "PROBE push panicked = true / PROBE end(Some(99)) -> result() = -2147483648". With the fix applied, `cargo test -p cyrup-core --lib` -> 37 passed.
```

## Acceptance Criteria

- [ ] `extract` is invoked before the `final_value` lock is taken in `FinalizingSink::push`, so no caller-supplied closure runs inside the critical section.
- [ ] All three `final_value.lock()` sites (push, end, result) recover from poisoning via `std::sync::PoisonError::into_inner` rather than silently discarding the value; a test asserts `result()` returns 99 after a panicking `extract` followed by `end(Some(99))`.
- [ ] The terminal-without-`end` path is resolved one of two ways, and the choice is stated in the module doc: either `push` clears `self.tx` on a terminal so `result()` resolves on the terminal item, or the docs at :23-25, :27 and :127-129 are rewritten to say a terminal only captures the value and `result()` resolves when the sink is ended or dropped.
- [ ] A regression test covers push-terminal-without-calling-`end` and asserts the documented outcome within a bounded `tokio::time::timeout` (no test may rely on the harness hanging).
- [ ] `cargo test -p cyrup-core --lib` passes and `cargo clippy -p cyrup-core --all-targets` exits 0 (use `Result::unwrap_or_else`, which the `unwrap_used` deny does not cover; extend the test module's existing `#[allow]` at :148 only for the deliberately-panicking closure).
- [ ] No serde-derived type is touched; `Shared` remains private and non-serializable.

## Provenance

Found by the cyrup-core hygiene audit workflow (2026-08-22), dimension-fanned and adversarially
verified. Severity **medium**, estimated effort **medium**.
