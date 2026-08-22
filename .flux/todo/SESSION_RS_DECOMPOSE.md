---
stage: new
status: done
updated: 2026-08-22 16:09
---

# Decompose cyrup-session-svc src/session.rs Into Submodules

## Description

decompose cyrup-session-svc `src/session.rs` into submodules based on logical separation of concerns

## Acceptance Criteria

- [ ] `src/session.rs` is split into submodules under `src/session/`, each grouped by a single logical concern
- [ ] Public API of the crate is unchanged (re-exported from `src/session/mod.rs` so existing call sites keep compiling)
- [ ] No behavioral changes — refactor only, no logic edits
- [ ] `cargo build` and `cargo test` pass for the crate
- [ ] `cargo clippy` reports no new warnings
