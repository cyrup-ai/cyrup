---
stage: qa
status: completed
updated: 2026-08-22 18:06
---

# Move the EventStream Alias Into the Module Named After It

## Description

`pub type EventStream<T>` lives at crates/cyrup-core/src/lib.rs:40-44 while crates/cyrup-core/src/event_stream.rs — the module named for it — contains only `Finalizing` (:26), `FinalizingSink` (:42), `FinalizingStream` (:87) and `finalizing_channel` (:130), and has to cross-link outward at :5 to `crate::EventStream<T>` to explain itself. All nine `EventStream` mentions in that module are doc prose; the alias has zero code consumers inside cyrup-core, so relocating it introduces no new intra-crate coupling. Moving it and re-exporting from the root keeps `cyrup_core::EventStream` resolving identically for all 164 downstream use sites, and no crate outside cyrup-core names the `event_stream::` module path. Purely a navigability fix with no behavioural weight; queue it only because it is provably zero-risk.

## Evidence

```
crates/cyrup-core/src/lib.rs:40-44 (alias; line 38 is the closing brace of the preceding `pub use tool::{...}`). `grep -rn "EventStream" crates/cyrup-core/src/` -> 11 hits, the only non-doc occurrence being lib.rs:44. `grep -rn "event_stream::" --include=*.rs crates/` -> one hit, lib.rs:29. `grep -rn "EventStream" --include=*.rs crates/ | wc -l` -> 164. Applied-then-reverted: `cargo clippy -p cyrup-core --all-targets` finished clean in 0.68s; `cargo check -p cyrup-provider` finished in 17.77s.
```

## Acceptance Criteria

- [ ] The doc comment and `pub type EventStream<T> = ...` are moved verbatim from crates/cyrup-core/src/lib.rs:40-44 into crates/cyrup-core/src/event_stream.rs above the `Finalizing` trait.
- [ ] `EventStream` is added to the existing `pub use event_stream::{...}` re-export at lib.rs:29-31, so `cyrup_core::EventStream` still resolves.
- [ ] The module header link at event_stream.rs:5 is changed from ``[`crate::EventStream<T>`]`` to the local ``[`EventStream<T>`]``.
- [ ] No `use` statement anywhere outside crates/cyrup-core/src/lib.rs is modified.
- [ ] `cargo clippy -p cyrup-core --all-targets` exits 0 and `cargo check --workspace` builds.

## Provenance

Found by the cyrup-core hygiene audit workflow (2026-08-22), dimension-fanned and adversarially
verified. Severity **low**, estimated effort **small**.
