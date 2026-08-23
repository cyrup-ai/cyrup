---
stage: exec
status: done
updated: 2026-08-23 00:18
---

# Spawn Lock Returns Ownership After A Discarded Failed Body Write, Leaving A Lock Any Peer Reads As Stale

> Source: `intercom-hygiene-audit` workflow. Severity **low**, effort **small**.
> Every claim below was produced by a finder agent and then reproduced by an independent
> adversarial verifier; findings that did not reproduce were dropped.

## Scope

- `crates/cyrup-intercom/src/transport/spawn.rs`

## Description

`acquire_spawn_lock` (crates/cyrup-intercom/src/transport/spawn.rs:405-424) exclusive-creates
`broker.spawn.lock`, writes the body `"<pid>\n<now>\n"` via `let _ =
file.write_all(body.as_bytes());` (:411) and then unconditionally `return true` (:413). That body
is the entire staleness record `is_spawn_lock_stale` reads back (:429-447), and I traced that
function against an empty file: `read_to_string` yields `Ok("")`, `"".trim().lines()` is empty, so
`pid` and `created_at` are both `None`, the `if let Some(pid)` guard is skipped and `match
created_at { None => true }` returns stale. So on a failed write the caller is told it owns the
lock while the on-disk lock is the one shape every peer classifies as stale and deletes
(:416-419), and both processes proceed as spawn owner.

## Why it matters

The bool returned by `acquire_spawn_lock` is the only mutual exclusion around broker spawning, and
one unchecked `write_all` makes it able to lie. If the write fails (ENOSPC, EDQUOT, EIO on the
agent dir), the process becomes a phantom owner holding a zero-byte lock that
`is_spawn_lock_stale` classifies as stale on sight, so a concurrent process unlinks it and claims
ownership too — and then also unlinks the survivor's lock in `release_spawn_lock`. The resulting
double spawn does not corrupt the socket (the loser is refused by `assert_no_live_broker`), but it
surfaces to a user as a spurious broker-start failure from a client whose broker is in fact
running, with nothing in the logs pointing at the discarded write. The condition is rare and the
existing tests never construct an empty lock body, so it would be diagnosed the hard way.

## Evidence

- crates/cyrup-intercom/src/transport/spawn.rs:409-413 — `let body = format!("{}\n{}\n", std::process::id(), now_ms()); let _ = file.write_all(body.as_bytes()); let _ = paths::restrict_intercom_runtime_file(lock_path); return true;` — the `return true` is unconditional, with nothing checking that the body reached disk
- crates/cyrup-intercom/src/transport/spawn.rs:429-447 — `is_spawn_lock_stale`: `let mut lines = contents.trim().lines(); let pid = lines.next()...; let created_at = lines.next()...;` then `if let Some(pid) = pid && !pid_alive(pid) { return true; } match created_at { None => true, Some(created) => now_ms().saturating_sub(created) > SPAWN_LOCK_STALE_MS }` — an empty body takes the `None => true` arm
- crates/cyrup-intercom/src/transport/spawn.rs:415-420 — the peer's `AlreadyExists` arm on a stale lock: `let _ = std::fs::remove_file(lock_path); continue;` and the loop re-`create_new`s successfully, so a second process also returns true
- crates/cyrup-intercom/src/transport/spawn.rs:69-72 — `ensure_broker` uses the bool as the sole mutual-exclusion decision: `if !acquire_spawn_lock(&lock_path) { return wait_for_broker_for(...) }`, otherwise it runs `spawn_owner`
- crates/cyrup-intercom/src/transport/spawn.rs:406-407 — the exclusive create itself is real (`OpenOptions::new().write(true).create_new(true)`) and its error IS handled; only the body write is unchecked
- crates/cyrup-intercom/src/transport/spawn.rs:402-404 — the function's own doc treats the body as load-bearing: "exclusive-create `broker.spawn.lock` (`O_EXCL`) with body `\"<pid>\\n<now>\\n\"`"
- Blast-radius check the original omitted: the existing unit tests at src/transport/spawn.rs:640-670 only exercise well-formed bodies (`acquire_spawn_lock` then `!is_spawn_lock_stale`, dead-pid stale, age-stale) — the empty-body case is untested; and the damage from a double owner is bounded because src/transport/spawn.rs:87 re-checks `is_broker_running_for` under the lock and crates/cyrup-intercom/src/broker/runtime_claim.rs:63 `assert_no_live_broker` makes the losing broker refuse to start rather than steal the incumbent's socket

## Required fix

Treat a failed body write as a failed acquisition instead of a successful one. At crates/cyrup-
intercom/src/transport/spawn.rs:411, replace `let _ = file.write_all(body.as_bytes());` with a
checked write that on `Err` logs (`tracing::warn!(error = %e, "failed to write intercom spawn lock
body")`), removes the empty lock file it just created, and returns `false`, so `ensure_broker`
falls through to `wait_for_broker_for` rather than spawning as a phantom owner. Keeping the `->
bool` signature leaves the call site at :69 untouched. Worth adding a unit test alongside the
existing ones at :640-670 that writes a zero-byte `broker.spawn.lock` and asserts
`is_spawn_lock_stale` is true, pinning the empty-body semantics this fix depends on.

## Acceptance Criteria

- [ ] The fix above applied as written; no scope beyond it.
- [ ] Port fidelity preserved — no `broker.ts`/`client.ts`/`paths.ts` citation dropped, no
      `[CYRUP-DELTA]` note removed, no ported branch collapsed.
- [ ] Baseline recorded before the change and matched after:
      `cargo clippy -p cyrup-intercom --all-targets`, `cargo test -p cyrup-intercom --lib`.
- [ ] `cargo build -p cyrup` still succeeds.
