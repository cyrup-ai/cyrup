---
stage: new
status: done
updated: 2026-08-22 20:10
---

# Unify The Four Copies Of The Poison-Safe Lock Helper

**Crate:** `crates/cyrup-session-svc` · **Severity:** medium · **Effort:** medium

## Description

The crate's most-repeated primitive has no canonical home. `rg -n 'fn lock' src/` returns four definitions of the same poison-recovery policy, each with its own arch-00 doc justification: `src/subscriber.rs:26` (free fn), `src/session/mod.rs:500` (inherent on AgentSession), `src/host_services.rs:673` (inherent on LiveHostServices), `src/guest_providers.rs:58` (inherent on GuestProviderRegistry, spelled with `PoisonError::into_inner`). 177 call sites depend on one of the four (96 `Self::lock(` under `src/session/`, 67 in host_services.rs, 8 in subscriber.rs, 6 in guest_providers.rs). Two more sites open-code it because their type has no helper to reach for: `src/provider_swap.rs:49-52` and `:57-62` as `match` blocks, and `src/host_services.rs:416`/`:421` inline `.lock().unwrap_or_else(|e| e.into_inner())` from `impl EditorTextMirror`, a different type from the one that owns the helper at :673. Since the workspace denies `clippy::unwrap_used` and `clippy::panic` and the crate opts in, every new `std::sync::Mutex` field must re-derive this rule from whichever neighbour the author happened to read.

## Acceptance Criteria

- [x] `src/sync.rs` defines a single `pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T>` with the poison policy documented once; `rg -n 'fn lock' crates/cyrup-session-svc/src/` returns exactly one definition.
- [x] `rg -c 'Self::lock\(' crates/cyrup-session-svc/src/` returns 0 — all 177 call sites migrated to `crate::sync::lock(..)`.
- [x] `rg -n 'into_inner' crates/cyrup-session-svc/src/` matches only `src/sync.rs`: the two `match` blocks in provider_swap.rs and the two inline sites at host_services.rs:416/:421 now call the helper.
- [x] `cargo clippy -p cyrup-session-svc --all-targets` gains no warnings and `cargo test -p cyrup-session-svc` still reports 311 passing.
- [x] `git diff --stat` shows the change is confined to lock-site call rewrites plus the new module (no behavioural edits mixed in).

## Findings

Each was produced by a survey agent and then adversarially checked by a separate verifier that tried to refute it. `OVERSTATED` means the finding is real but the verifier corrected its scope or severity — the corrected values are the ones below.

### Collapse the four private copies of the poison-safe `lock()` helper into one crate-level helper

`CONFIRMED` · severity **medium** · effort **medium** · dimension `consistency`

**Evidence.** src/subscriber.rs:26-28, src/session/mod.rs:500-502, src/host_services.rs:673-675, src/guest_providers.rs:58-61 are four definitions of the same poison-recovery policy, each with its own arch-00 doc justification. 177 call sites depend on one of the four. src/provider_swap.rs:49-52 and :57-62 open-code it as `match`; src/host_services.rs:416 and :421 inline `.lock().unwrap_or_else(|e| e.into_inner())` (they are in `impl EditorTextMirror`, which has no `lock` of its own). Workspace denies clippy::unwrap_used/panic (Cargo.toml:97-101, crate opts in via Cargo.toml:11-12 `[lints] workspace = true`), so every new `std::sync::Mutex` field must re-derive this.

**Why it matters.** This is the crate's most-repeated primitive and it has no canonical home. Four spellings of one policy plus four open-coded sites means a reviewer cannot tell whether a given lock site is intentional or a fifth reinvention, and any new type holding a `Mutex` re-derives the rule from whichever neighbour the author happened to read (which is demonstrably how the `match` forms in provider_swap.rs got written — that type has no helper to reach for).

**Fix.** Add `pub(crate) fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> { m.lock().unwrap_or_else(std::sync::PoisonError::into_inner) }` in a new `src/sync.rs`, delete the four private definitions, and sed the call sites (`Self::lock(` -> `crate::sync::lock(`; `self.lock()` in guest_providers.rs -> `crate::sync::lock(&self.providers)`). Rewrite provider_swap.rs:49-62's two `match` blocks and host_services.rs:416/:421 to call it too, so the poison policy is stated exactly once.
