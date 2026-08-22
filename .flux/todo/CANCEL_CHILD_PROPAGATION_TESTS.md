---
stage: new
status: done
updated: 2026-08-22 17:24
---

# Test Root-to-Child Cancellation and cancel() Idempotency

## Description

`RunCancel::child()` (crates/cyrup-core/src/cancel.rs:29-31) is the sole mechanism by which a run abort reaches hooks and tool execution — `grep -rn "\.child()" --include=*.rs crates/` finds 10 production call sites, all in cyrup-agent/src/agent.rs (191, 458, 659, 720, 778, 836, 1100, 1265, 1423, 1570) — yet nothing in the workspace exercises root-to-child propagation. The only cancel tests (cancel.rs:48-67) touch new()/cancel()/is_cancelled()/run_until on the root; the other RunCancel test sites in cyrup-agent either use `token()` or construct a RunCancel that is never cancelled. The crate's own doc at cancel.rs:28 states the child contract and cancel.rs:33 states idempotency (R-02-045); neither is pinned. `child()` is a one-line delegation to tokio_util's `child_token()`, so this is cheap regression insurance on a stated contract, not a live defect. Add the tests to the existing `mod tests` in cancel.rs.

## Evidence

```
crates/cyrup-core/src/cancel.rs:29-31 (`pub fn child(&self) -> CancelToken { self.root.child_token() }`), existing tests at :48-67. `cargo test -p cyrup-core -- --list` -> "36 tests, 0 benchmarks", only two cancel::tests entries. `grep -rn "\.child()" --include=*.rs crates/` -> 13 hits: 10 production sites in cyrup-agent/src/agent.rs, 3 comments, 0 tests.
```

## Acceptance Criteria

- [ ] A test asserts that after `rc.cancel()`, both a token from `rc.child()` and a token from `rc.token()` report `is_cancelled() == true`, and that `child.run_until_cancelled(std::future::pending::<()>()).await` resolves to `None`.
- [ ] A test asserts that cancelling a child alone leaves `rc.is_cancelled() == false` and a freshly taken `rc.child()` un-cancelled.
- [ ] A test calls `rc.cancel()` twice and asserts `is_cancelled()` stays true and `run_until` still yields `None`, pinning R-02-045.
- [ ] All new tests live in crates/cyrup-core/src/cancel.rs's existing `mod tests` and use `#[tokio::test(flavor = "current_thread")]`.
- [ ] `cargo test -p cyrup-core -- --list` reports 39 tests (was 36) and `cargo test -p cyrup-core` passes.
- [ ] No production code in cancel.rs is modified.

## Provenance

Found by the cyrup-core hygiene audit workflow (2026-08-22), dimension-fanned and adversarially
verified. Severity **medium**, estimated effort **small**.
