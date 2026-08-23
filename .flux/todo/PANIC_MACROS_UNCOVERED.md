---
stage: exec
status: done
updated: 2026-08-23 00:16
---

# No-Panic Deny Wall Misses unreachable!/todo!/unimplemented!

> Source: `intercom-hygiene-audit` workflow. Severity **medium**, effort **small**.
> Every claim below was produced by a finder agent and then reproduced by an independent
> adversarial verifier; findings that did not reproduce were dropped.

## Scope

- `crates/cyrup-intercom/src/lib.rs`
- `Cargo.toml`
- `crates/cyrup-intercom/src/inbound.rs`

## Description

The crate's no-panic wall is exactly four lints — `clippy::unwrap_used`, `expect_used`, `panic`,
`indexing_slicing` — denied in both `crates/cyrup-intercom/src/lib.rs:14` and the
`[workspace.lints.clippy]` table at `/home/user/cyrup/Cargo.toml:97-101`. I re-ran both probes
myself: all four lints fire on injected violations, and `unreachable!` / `todo!` /
`unimplemented!` compile with ZERO diagnostics in the same production file. `clippy::panic` covers
only the `panic!` macro; `clippy::unreachable` / `todo` / `unimplemented` are separate restriction
lints enabled nowhere in the workspace. lib.rs:12-13 states the no-panic policy (arch-00 §8) is
"enforced crate-wide", and `src/broker/presence.rs:129-130` documents the author deliberately
choosing a silent no-op over a `panic!`/`unreachable!` at a broker frame-handling arm — the
crate's own reasoning treats both macros as equally forbidden while the tooling enforces only one.
The gap has already been exercised silently once: the `unreachable!` at `src/inbound.rs:1026`
needed no `#[allow]`. A future contributor writing `unreachable!()` in a broker dispatch arm gets
a clean build and a process whose crash takes down every local session sharing the broker.

## Why it matters

This is a stated intent the tooling does not fulfil, not a style preference. lib.rs:12-13 asserts
the no-panic policy is enforced crate-wide, and presence.rs:129-130 shows the author reasoning as
though `unreachable!` were as forbidden as `panic!` — but the enforcement mechanism covers only
one of the four panicking macros. The protected asset is a shared broker process: presence.rs's
own comment names the consequence, "taking the whole broker down", i.e. every local session
connected to that socket. The hole is not theoretical — inbound.rs:1026 already passed through it
with no diagnostic and no `#[allow]`, which is exactly how the next one lands in production
dispatch code unnoticed. Nothing here touches ported pi-intercom logic or a CYRUP-DELTA; it is the
crate's own lint configuration.

## Evidence

- crates/cyrup-intercom/src/lib.rs:14 — verified verbatim via `sed -n '14p'`: `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]`. lib.rs:12-13 claims the no-panic policy is "enforced crate-wide via `[workspace.lints]`".
- /home/user/cyrup/Cargo.toml:97-101 — `grep -n -A20 '\[workspace.lints'` shows the clippy table holds exactly four keys (unwrap_used, expect_used, panic, indexing_slicing = "deny"), immediately followed by `[workspace.dependencies]` at :103. No unreachable/todo/unimplemented/exit entry.
- PROBE A (wall is live) — I appended `probe_wall` to crates/cyrup-intercom/src/cwd.rs using `Some(1).unwrap()`, `Some(2).expect("probe")`, `v[0]`, `panic!("probe")` and ran `cargo clippy -p cyrup-intercom --lib`: 4 errors at cwd.rs:141,142,143,144 — 'used `unwrap()` on an `Option` value', 'used `expect()` on an `Option` value', 'indexing may panic', '`panic` should not be present in production code'.
- PROBE B (the gap) — I appended `probe_gap` to the SAME production file matching on `0 => unreachable!("probe"), 1 => todo!("probe"), 2 => unimplemented!("probe"), 3 => std::process::exit(1)` and re-ran `cargo clippy -p cyrup-intercom --lib`: output was only `Finished dev profile`, zero diagnostics on cwd.rs. Working tree restored; `git status --porcelain` empty.
- crates/cyrup-intercom/src/broker/presence.rs:129-130 — read directly: "The arm is written as a no-op rather than a `panic!`/`unreachable!` so a future refactor that drops the pre-validation degrades to 'ignored' instead of taking the whole broker down."
- crates/cyrup-intercom/src/inbound.rs:1026 — `unreachable!("asserted equal to Deliver above")`, inside `#[cfg(test)] mod tests` opened at :627-628 whose inner attribute at :629 is `#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]` — no panic-family allow, because none of the four denied lints see it. `awk` over lines 600-1030 confirms no intervening `mod` boundary, so :629's allow does govern :1026.
- ENUMERATION — `grep -rnE 'unreachable!|todo!|unimplemented!' src/` returns exactly 2 hits crate-wide: src/broker/presence.rs:129 (the doc comment) and src/inbound.rs:1026 (the sole real call site). `grep -rn 'process::exit' src/` returns exactly 1 hit: src/bin/cyrup_intercom_child_fixture.rs:57.
- FIX VALIDATION — I applied the proposed deny list to lib.rs:14 and ran `cargo clippy -p cyrup-intercom --all-targets --features test-fixtures`: all four added lint names are valid (no 'unknown lint' warning) and exactly ONE new error fired in this crate, at src/inbound.rs:1026, alongside the pre-existing 3 warnings (broker/receipts.rs:243,244; broker/test_support.rs:55). Reverted via `git checkout`.

## Required fix

Add `unreachable = "deny"`, `todo = "deny"`, and `unimplemented = "deny"` to the
`[workspace.lints.clippy]` table at /home/user/cyrup/Cargo.toml:97-101, and restate them in the
`#![deny(...)]` at crates/cyrup-intercom/src/lib.rs:14 to match the existing four. Put them in the
workspace table, not lib.rs alone: lib.rs's inner attribute governs only the lib crate root,
whereas the package's `[lints] workspace = true` (crates/cyrup-intercom/Cargo.toml:11-12) reaches
the bin target too. Exactly one allow is then required, which I verified by applying the change:
add `clippy::unreachable` to the existing test-module inner allow at crates/cyrup-
intercom/src/inbound.rs:629, covering the sole call site at inbound.rs:1026. Do NOT touch
broker/presence.rs's no-op arm; its comment simply becomes accurate. Two corrections to the
original proposal, both verified by re-running: (1) `clippy::exit` is NOT needed and buys nothing
here — I added `exit = "deny"` to the workspace table and re-ran clippy with `--features test-
fixtures`; the fixture bin produced zero diagnostics, because the lone `std::process::exit` call
sits directly inside `fn main` (cyrup_intercom_child_fixture.rs:55-58), which `clippy::exit`
exempts by design. No `#[allow(clippy::exit)]` is required anywhere. (2) That call is at line 57,
not line 63 as originally cited.

## Acceptance Criteria

- [ ] The fix above applied as written; no scope beyond it.
- [ ] Port fidelity preserved — no `broker.ts`/`client.ts`/`paths.ts` citation dropped, no
      `[CYRUP-DELTA]` note removed, no ported branch collapsed.
- [ ] Baseline recorded before the change and matched after:
      `cargo clippy -p cyrup-intercom --all-targets`, `cargo test -p cyrup-intercom --lib`.
- [ ] `cargo build -p cyrup` still succeeds.
