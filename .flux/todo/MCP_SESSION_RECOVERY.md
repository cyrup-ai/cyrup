---
stage: new
status: done
updated: 2026-08-22 06:00
---

# MCP-135: withSessionRecovery Retry Wrapper

## Description

The whole wrapper is absent (`high`, `13c`, `missing` —
`docs/gap-analysis/13-cyrup-mcp-STATUS.md:646`). The second half of wave 6, and it pairs with
MCP-119: discovery is the main thing that needs recovering.

Upstream's `withSessionRecovery` is a sequence where **order is the specification**, so port it
against the TypeScript rather than from the summary:

- disabled / not-connected preconditions checked first
- `hadSessionId` captured **before** the call, not after
- the config re-read after a failure must be the **live** config, not the captured one
- 401 credential-cache invalidation runs **before** the `isTerminatedSession` gate
- exactly one retry — not a loop, not zero
- the `onNeedsAuth` hook fires on the right arm

Getting the order wrong yields something that passes a happy-path test and silently never
recovers, which is the failure mode this unit exists to prevent.

Note Rust/JS divergence that bit the transport work in PR #30: a dropped Rust future runs
nothing, where a JS promise completes regardless of who holds it. Any "capture before, compare
after" logic must not live in a future that can be cancelled between the two points.

## Acceptance Criteria

- [ ] Preconditions, capture order, live re-read, 401-before-terminated-gate, single retry and `onNeedsAuth` all match upstream
- [ ] A test asserts the retry happens exactly once, and a second asserts the 401 path invalidates credentials before the terminated-session check
- [ ] A test covers cancellation between capture and comparison
- [ ] `MCP-135`'s row in `13-cyrup-mcp-STATUS.md` is updated
- [ ] `cargo nextest run --workspace` and `cargo clippy --workspace --all-targets` are clean
