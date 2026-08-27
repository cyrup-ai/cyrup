---
stage: qa
status: completed
priority: MEDIUM
tool: all
source: exec follow-up — self-reported by the mutation-lock executor
updated: 2026-08-27 21:51
---

# Outstanding: one stale doc rationale left behind by the fix

The witness itself is **accepted**. `guard_entries` is per-instance, incremented as the
first statement of `guard`'s body above every `.await`, read off the same `Arc` the test
hands to its tool; assertions (1) and (2) are untouched; both rejected dead ends were
correctly avoided; GREEN is 7/7 here. Do not reopen any of that.

One item remains, and it is in the file this task edited.

## `registration_is_held`'s doc now states a reason that is false

[`crates/cyrup-tools/src/lock.rs:46-48`](../../../crates/cyrup-tools/src/lock.rs):

```rust
/// Test-only observer on the registration chain, so the tests below can assert on its STATE
/// instead of on a sleep. `pub(crate)` because `crate::tests::mutation_lock_is_first_await` needs
/// it too; `cfg(test)` because nothing outside the suite may ever reach the static.
#[cfg(test)]
pub(crate) fn registration_is_held() -> bool {
```

This task **removed** that consumer — assertion (3) in
`crates/cyrup-tools/src/tests/mutation_lock_is_first_await.rs` no longer imports or calls
`registration_is_held`, and the import at `:19` is now `use crate::lock::FileMutationLocks;`
alone. Verified: the only remaining reference in the workspace is
`crates/cyrup-tools/src/lock.rs:332`, inside `lock.rs`'s own `mod tests`.

So the sentence is wrong on both halves it asserts:

1. `crate::tests::mutation_lock_is_first_await` does **not** need it any more — this task is
   precisely the change that stopped it needing it, and the new field doc 76 lines below
   (`lock.rs:122-129`) says so explicitly.
2. `pub(crate)` is no longer load-bearing. The sole caller is `mod tests` at `lock.rs:272`,
   which does `use super::*` and therefore reaches a module-private item.

This is the same class of defect the parent task
(`MEDIUM-same-path-mutation-lock-is-not-granted-in-dispatch-order.md`, item 2) is still open
on — a doc cross-reference that outlived the thing it pointed at — in the same function
family, and this file's docs are treated as the rationale of record.

### Required

- Rewrite the rationale sentence to state the real reason the observer still exists: it is
  the witness for `the_registration_chain_spans_key_resolution` (`lock.rs:332`), whose
  property — *the chain is held **across key resolution*** — is genuinely about the
  process-global static and so cannot be served by `guard_entries`. Say that the
  first-`.await` test that used to share it now uses the per-instance counter instead, and
  keep the existing pointer to the hazard so the next reader does not re-adopt the `try_lock`
  for a non-global property.
- Narrow the visibility to module-private unless that breaks the build. Keep `#[cfg(test)]`.
- Do **not** delete the function: `lock.rs:332` still asserts on it.

### Do not

- Do not touch `guard_entries`, the increment at `lock.rs:242-245`, the accessor at
  `lock.rs:210-213`, the field at `lock.rs:130`, the constructor at `lock.rs:167`, or any of
  the three assertions in `mutation_lock_is_first_await.rs`. All verified correct.
- Do not expand into `the_registration_chain_spans_key_resolution`'s own sampling defect —
  the original task already scoped that out and that remains right.

## Verification

Doc-only plus a visibility narrowing, so the gate is that nothing moved:

```bash
cd /home/user/cyrup
cargo test -p cyrup-tools --lib 2>&1 | tail -3   # 306 passed, 0 failed
```

## Definition of done

1. `lock.rs:46-48`'s rationale describes the actual remaining consumer (`lock.rs:332`) and no
   longer claims `crate::tests::mutation_lock_is_first_await` needs the function.
2. `registration_is_held` still exists, still `#[cfg(test)]`, still called at `lock.rs:332`.
3. `cargo test -p cyrup-tools --lib` — 306 passed, 0 failed.

## Note for the orchestrator, not work for this task

The parent's item-1 "Done when" also asks that the RED/GREEN counts be **recorded in the exec
notes**. Nothing in the tree records them; the counts (RED 20/20 write, RED 20/20 edit, GREEN
20/20) currently live only in the executor's report. Book that against the parent when closing
it, not here.
