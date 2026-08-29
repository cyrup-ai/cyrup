---
stage: qa
status: needs-rework
updated: 2026-08-29 02:24
---

# Native modifier probe — one wrong FFI type, one undemonstrated claim

The probe, its registration and the Windows `[CYRUP-DELTA]` are **complete**. Do not redo them.

Independently verified against `objc2-core-graphics-0.3.2`'s generated binding (in the registry
cache at `src/generated/CGEventTypes.rs`), so this does not need re-checking: `MaskShift` = 131072 =
`0x0002_0000`, `MaskControl` = 262144, `MaskAlternate` = 524288, `MaskCommand` = 1048576 — all four
match the constants in `main.rs` — and `CGEventFlags` is `u64`, matching the declared return type.
Build 0 errors / 0 warnings; `cargo clippy --workspace --all-targets` 0; `cargo test --workspace`
8300 passed / 0 failed; `cyrup-tui` keeps `#![forbid(unsafe_code)]`; no Windows probe is registered.

## 1. `CGEventSourceStateID` is signed, and the comment says otherwise

`crates/cyrup/src/main.rs:117-118,130`:

```rust
/// `kCGEventSourceStateCombinedSessionState` — `CGEventSourceStateID` is a `uint32_t` enum.
const COMBINED_SESSION_STATE: u32 = 0;
...
fn cg_event_source_flags_state(state_id: u32) -> u64;
```

The binding declares `pub struct CGEventSourceStateID(pub i32)`, and `pub const Private: Self =
Self(-1)` settles the signedness — `-1` is not expressible in `u32`.

Harmless today: the only value passed is `0`, which has an identical representation in both types and
travels in the same register class on x86-64 and AArch64. It is still a wrong type at an FFI boundary,
and the doc comment states a fact the authoritative binding contradicts, which is the exact failure
this branch exists to stop.

- [ ] `COMBINED_SESSION_STATE` is `i32`
- [ ] the extern takes `state_id: i32`
- [ ] the doc comment says `int32_t` (or drops the width claim), and no comment in the module asserts
      an unsigned state id
- [ ] `cargo build --workspace --all-targets` — 0 errors, 0 warnings
- [ ] `cargo clippy --workspace --all-targets` — 0
- [ ] the module still typechecks for `aarch64-apple-darwin` — extract it verbatim and
      `rustc --edition 2024 --target aarch64-apple-darwin --emit=metadata` under `#![deny(warnings)]`,
      as the previous pass did (the target is already installed)

## 2. The Apple Terminal behaviour is still unproven

The definition of done requires *"Shift+Enter inserts a newline in Apple Terminal instead of
submitting."* That has not been demonstrated and cannot be from this container: the full binary will
not cross-compile for macOS (`aws-lc-sys` and `zstd-sys` need a macOS SDK to build their C), and the
behaviour needs a real Apple Terminal session.

What IS established is that the code typechecks for `aarch64-apple-darwin` and that the registration
expression coerces correctly. What is NOT established is that `CGEventSourceFlagsState` resolves at
link time and that the rescue fires end to end.

- [ ] Run cyrup in Apple Terminal on macOS, press Shift+Enter, confirm a newline is inserted and the
      message is not submitted
- [ ] Confirm the binary links — `#[link(name = "CoreGraphics", kind = "framework")]` is unverified
      until something links it

This needs macOS hardware. It is not a code defect and nothing above is blocked on it, but the
project's standing rule is that TUI work is not done until it is run in a terminal, so it stays open
rather than being marked complete.

## Constraints

- Attributes, types and comments only for item 1. Do not touch the mask constants — they are verified
  correct — the match arms, or the `// SAFETY:` comment.
- A Windows probe IS registered, by decision on 2026-08-29 after the analysis was weighed against
  the cost of being wrong. The `[CYRUP-DELTA]` records both the analysis and that decision.
- Do not weaken `cyrup-tui`'s `#![forbid(unsafe_code)]`.
