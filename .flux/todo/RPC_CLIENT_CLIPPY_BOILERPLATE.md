---
stage: new
status: done
updated: 2026-08-22 18:45
---

# Clear the 6 Clippy Warnings in rpc_client.rs and Collapse Its Lock Boilerplate

## Problem

`cargo clippy -p cyrup-modes --all-targets --no-deps` emits exactly 6 warnings, **all** in `crates/cyrup-modes/src/rpc_client.rs`. Every other file in the crate is clean, so the crate can never return a clean lint run and a genuinely new warning has to be spotted inside standing noise. There is no CI workflow, so the local run is the only gate. The crate inherits the workspace policy (`crates/cyrup-modes/Cargo.toml:11-12` -> `[lints] workspace = true`; `/home/user/cyrup/Cargo.toml:97-101` denies `unwrap_used`/`expect_used`/`panic`/`indexing_slicing`), and `rpc_client.rs` contains **zero** `#[allow(...)]` attributes, so none of the six sites is a documented deliberate exception.

The six sites:

| line | lint | shape |
|---|---|---|
| `rpc_client.rs:342` | `clippy::single_match` | `match serde_json::from_value::<RpcResponse>(data)` with an empty `Err(_) => {}` arm |
| `rpc_client.rs:617` | `clippy::collapsible_if` | `self.reader_task.lock()` block in `pub async fn stop(&self)` |
| `rpc_client.rs:648` | `clippy::collapsible_if` | `self.stderr_task.lock()` block in `pub async fn stop(&self)` |
| `rpc_client.rs:1056` | `clippy::collapsible_if` | `event_type(event) == Some(AGENT_SETTLED)` + `if let Some(tx) = tx.take()` in `subscribe_collect` |
| `rpc_client.rs:1162` | `clippy::collapsible_if` | `self.reader_task.lock()` block in `impl Drop for RpcClient::drop` |
| `rpc_client.rs:1167` | `clippy::collapsible_if` | `self.stderr_task.lock()` block in `impl Drop for RpcClient::drop` |

617/648/1162/1167 are four verbatim copies of one five-line block:

```rust
if let Ok(mut slot) = self.reader_task.lock() {
    if let Some(handle) = slot.take() {
        handle.abort();
    }
}
```

Separately, and adjacent in the same file, the four-line idiom

```rust
match <mutex>.lock() { Ok(g) => g, Err(p) => p.into_inner() }
```

appears verbatim at twelve sites — `rpc_client.rs:277-280, 291-294, 301-304, 312-315, 335-338, 418-421, 437-440, 572-575, 666-669, 1030-1033, 1047-1050, 1104-1107` — plus two `Err(p) => p.into_inner().len()` variants at `:689-692` and `:698-701` (both `#[cfg(test)]`). That is ~50 lines and the single most repeated shape in the file. It also drives the file's deepest indentation: `rpc_client.rs:336-337` and `:573-574` sit at 24 spaces and are pure boilerplate. All sites are `std::sync::Mutex` (`rpc_client.rs:86`), not tokio. This is **not** a port shape — pi has no locks at all; the module doc frames locking as "mechanism gap 2", cyrup-only (`rpc_client.rs:49-53`) — so collapsing it costs no port fidelity, and the compact spelling is already accepted in this file at `rpc_client.rs:273` (`.unwrap_or_else(|p| p.into_inner().clone())`).

## Fix

All edits are behaviour-preserving and confined to `crates/cyrup-modes/src/rpc_client.rs`.

1. **342** — rewrite as `if let Ok(response) = serde_json::from_value::<RpcResponse>(data) { let _ = tx.send(response); }`. Clippy explicitly warns "you might want to preserve the comments from inside the `match`": **lift both existing pi-parity comments** (the "receiver may already be gone / resolve on a settled promise is a no-op" note and the "malformed response line ... `JSON.parse` catch swallows a bad line" note) into a single comment above the `if`. Dropping them loses the upstream justification.
2. **617 / 648 / 1056 / 1162 / 1167** — use the edition-2024 let-chain clippy suggests, e.g. `if let Ok(mut slot) = self.reader_task.lock() && let Some(handle) = slot.take() { handle.abort(); }` (the second `let` borrowing the `slot` bound by the first is valid). Put the opening brace on its own line for readability.
3. Since 617/648/1162/1167 are four copies of one block, prefer extracting a small private helper taking `&Mutex<Option<JoinHandle<()>>>` and calling it four times — that removes the duplication as well as the lint.
4. **Lock boilerplate** — add one free function near the other module-level helpers under the `// Free helpers` banner (`rpc_client.rs:1175-1226`):
   ```rust
   fn lock_ignoring_poison<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
       m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
   }
   ```
   Replace each of the twelve four-line matches with one call (`let mut map = lock_ignoring_poison(&self.inner.pending);`) and the two `.len()` sites with `lock_ignoring_poison(&self.inner.listeners).len()`. `PoisonError::into_inner` is exactly what every arm already does, so behaviour is identical; `clippy::unwrap_used` does not fire on `unwrap_or_else`. Net ~36 lines removed and the two deepest nesting sites flattened.

Do not change any pi-parity comment text other than relocating the two at `:342`.

## Acceptance Criteria

- [ ] `cargo clippy -p cyrup-modes --all-targets --no-deps` completes with zero warnings
- [ ] `grep -c 'into_inner()' crates/cyrup-modes/src/rpc_client.rs` shows the poison-recovery idiom collapsed to the single helper (no remaining four-line `match <mutex>.lock() { Ok(g) => g, Err(p) => p.into_inner() }` blocks)
- [ ] The five-line `if let Ok(mut slot) = self.{reader,stderr}_task.lock() { if let Some(handle) = slot.take() { handle.abort(); } }` block no longer appears four times
- [ ] Both pi-parity comments from the old `match` at rpc_client.rs:342 are still present in the file, above the replacement `if let`
- [ ] No `#[allow(...)]` attribute was added to rpc_client.rs to silence any of the six lints
- [ ] `cargo test -p cyrup-modes` passes with the same result set as before the change (75 tests; the pre-existing `rpc_cycle_model_spans_the_full_auth_filtered_registry` failure is out of scope and owned by TEST_FAILURES.md)

## Source

- Identified by the cyrup-modes hygiene audit (workflow `cyrup-modes-hygiene-audit`)
- Severity: medium | Size: small
