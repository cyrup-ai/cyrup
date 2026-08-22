---
stage: new
status: done
updated: 2026-08-22 17:24
---

# Add Debug to RunCancel, FinalizingSink and FinalizingStream

## Description

`cargo clippy -p cyrup-core --lib -- -W missing_debug_implementations -A clippy::all` reports exactly three public types with no `Debug` impl and nothing else: `RunCancel` (crates/cyrup-core/src/cancel.rs:14), `FinalizingSink` (event_stream.rs:42) and `FinalizingStream` (event_stream.rs:87). `RunCancel` is a `#[derive(Clone, Default)]` newtype over a `CancellationToken` that is itself `Debug`, so the omission is accidental, and because `RunCancel` is embedded in public downstream types (cyrup-ext/src/host_runtime.rs, cyrup-agent/src/agent.rs) the missing impl blocks `#[derive(Debug)]` on every holder. The two event_stream types cannot derive (their `Shared` holds boxed closures) and need small manual impls with no `T: Debug`/`F: Debug` bounds, so downstream aliases over `StreamEvent`/`AgentEvent` stay usable. Add the crate-level lint so the gap cannot silently reopen. Compile-time only — no serde impl reads `Debug` and no wire bytes move.

## Evidence

```
$ cargo clippy -p cyrup-core --lib -- -W missing_debug_implementations -A clippy::all
warning: type does not implement `std::fmt::Debug` --> crates/cyrup-core/src/cancel.rs:14:1
warning: type does not implement `std::fmt::Debug` --> crates/cyrup-core/src/event_stream.rs:42:1
warning: type does not implement `std::fmt::Debug` --> crates/cyrup-core/src/event_stream.rs:87:1
(exactly three; the full fix was written and re-run clean in 0.54s, then reverted)
```

## Acceptance Criteria

- [ ] crates/cyrup-core/src/cancel.rs:13 derives `Debug` alongside `Clone` and `Default`.
- [ ] `FinalizingSink` and `FinalizingStream` have manual `Debug` impls that are unconditional in `T` and `F` (no `Debug` bounds on the type parameters) and use `finish_non_exhaustive()`.
- [ ] `#![warn(missing_debug_implementations)]` is added next to the existing `#![forbid(unsafe_code)]` in crates/cyrup-core/src/lib.rs; the lint is NOT lifted to the workspace in this task.
- [ ] `cargo clippy -p cyrup-core --lib -- -W missing_debug_implementations -A clippy::all` reports zero warnings.
- [ ] `cargo clippy -p cyrup-core --all-targets` exits 0 and `cargo check -p cyrup-agent -p cyrup-ext` still builds.

## Provenance

Found by the cyrup-core hygiene audit workflow (2026-08-22), dimension-fanned and adversarially
verified. Severity **medium**, estimated effort **small**.
