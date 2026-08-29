---
stage: new
status: done
updated: 2026-08-29 03:04
---

# Unix-Only Crates Are Ungated, So The Windows Build May Not Resolve At All

## Description

`crates/cyrup-mcp/Cargo.toml:217` carries the warning in its own words — **"`nix` would take Windows
out of the build entirely"** — and gates it behind `[target.'cfg(unix)'.dependencies]`.
`cyrup-intercom` and `cyrup-modes` do the same. **`cyrup-ext-subagents` does not.**

| crate | dependency | section |
|---|---|---|
| `cyrup-ext-subagents` | `nix`, `libc` | `[dependencies]` — **ungated** |
| `cyrup-tui` | `rustix` (`std`, `event`) | `[dependencies]` — **ungated** |
| `cyrup` | `libc` | `[dependencies]` — ungated |
| `cyrup-modes` | `libc` | `[dependencies]` — ungated |
| `cyrup-intercom` | `nix` | `[target.'cfg(unix)'.dependencies]` ✅ |
| `cyrup-mcp` | `nix` | `[target.'cfg(unix)'.dependencies]` ✅ |
| `cyrup-modes` | `nix` | `[target.'cfg(unix)'.dependencies]` ✅ |

`nix` is a Unix-API crate and does not build for Windows. If `cyrup-ext-subagents` pulls it
unconditionally, no Windows target resolves — which would make every other Windows question moot,
including the Shift+Enter probe this was found beside.

`libc` does build on Windows (a thin shim), and `rustix` supports Windows only for some feature sets;
`event` is the one to check. Neither is assumed here — establish each.

## Could not be proven from this container

`cargo check --workspace --target x86_64-pc-windows-msvc` and the same for
`-p cyrup-ext-subagents` both fail before reaching any Rust: `aws-lc-sys` and `zstd-sys` run C
builds that need a real Windows toolchain (`unknown type name 'pthread_rwlock_t'`). The Cargo.toml
facts above are certain; the *consequence* is inferred and needs a real Windows target or CI runner.

## Required

1. Establish whether a Windows target resolves today. A CI job on `windows-latest` running
   `cargo check --workspace` settles it and keeps it settled.
2. Gate every Unix-only dependency behind `[target.'cfg(unix)'.dependencies]`, following the pattern
   `cyrup-mcp` already sets, and make the code behind them `cfg`-conditional to match.
3. For each of the 276 `cfg(unix)` sites against 38 `cfg(windows)`, the question is not "add a
   Windows arm" but "does this path have a Windows equivalent, or is the feature Unix-only by
   design?" Record which, rather than leaving the imbalance unexplained.

## Acceptance Criteria

- [ ] `cargo check --workspace --target x86_64-pc-windows-msvc` resolves and compiles, or the exact
      blocking crates are named with a decision recorded for each
- [ ] No Unix-only crate appears in a plain `[dependencies]` section
- [ ] CI runs a Windows check so this cannot regress silently
- [ ] `cargo build --workspace --all-targets` on Linux — 0 errors, 0 warnings
- [ ] `cargo test --workspace --no-fail-fast` — no regression (baseline: 8300 passed)

## Constraints

- Do not delete Unix functionality to make Windows build. Gate it.
- Do not add a Windows arm that cannot be exercised; if a path has no Windows equivalent, say so in a
  `[CYRUP-DELTA]` and leave it Unix-only.
- Workspace lints deny `unwrap_used`, `expect_used`, `panic`, `indexing_slicing`.

## Note on OQ-5

`docs/PARITY-PLAN.md:1389` defers "whether Windows support means anything" and records the 161-vs-6
`cfg` imbalance (now 276-vs-38) as evidence it may be a property of the whole port. **This task is
upstream of that question, not gated by it:** if the workspace does not resolve for Windows, OQ-5
cannot be answered either way, because there is nothing to evaluate.

## Source

Found while asking whether the Shift+Enter modifier probe mattered for Windows users. It may not; this
does.
