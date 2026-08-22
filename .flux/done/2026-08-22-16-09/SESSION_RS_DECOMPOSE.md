---
stage: qa
status: completed
updated: 2026-08-22 18:07
---

# Two Latent Doc Links Still Broken By The session.rs Split

## QA verdict: 9/10 — needs rework

The rework hit **every stated target exactly** — 8 warnings under `src/session/`, 0 unresolved
links, 19 crate-wide, 0 check/clippy, 311 tests. The reasoning was sound and empirically validated,
and the correction of the regression count from 7 to 5 was right.

But the definition of done measured the **normal** doc build, and `cargo doc` does not link-check
items it does not document. Two more links that resolved before the split do not resolve now; they
were hiding below that threshold. Same defect class, same fix, one flag away from visible:

```bash
cargo doc -p cyrup-session-svc --no-deps --document-private-items --message-format short 2>&1 \
  | grep 'src/session/' | grep 'unresolved link'
```

This is the second round where the sweep was narrower than the defect. Fix these two, then re-run
the sweep at full width before declaring done.

### Verified complete — do NOT redo

- The decomposition: 21 files under `src/session/`, largest 696 lines, public API and `lib.rs`
  untouched, workspace builds, **311 tests pass**.
- `cargo check` / `cargo clippy` (`--lib`, `--no-default-features`, `--all-targets`) — **0 warnings
  under `src/session/`**.
- The 5 visible doc regressions are gone; `cargo doc --workspace --no-deps` is back to **19** for
  the crate, matching the [`CARGO_DOC_WARNINGS.md`](CARGO_DOC_WARNINGS.md) baseline, with the same
  warning classes it had before the split.
- The three `#[cfg(doc)]` imports in `types.rs`, `accessors.rs` and `mod.rs` are correct and cost
  nothing in a normal build. The idiom is proven — reuse it below.
- `model.rs`'s module-doc `[`ProviderSwap`]` → `[`crate::ProviderSwap`]` qualification.

---

## The two remaining breaks

Both are doc comments carried over from `session.rs` that name something which **was** in scope
there and is not in scope in the module it landed in. Verified against the original's import block
and item list:

| Site | Link | Why it resolved before |
| --- | --- | --- |
| `session/compaction.rs:412` | `` [`BashCancelGuard`] `` | the struct was **defined in** `session.rs` |
| `session/mod.rs:292` | `` [`SessionServiceError::NoRuntimeHost`] `` | `session.rs:35` had `use crate::error::SessionServiceError;` |

Neither name appears in *code* in the file that needs it (verified: 0 occurrences each), so both take
the same `#[cfg(doc)]` treatment already used three times in this crate.

### Fix A — `src/session/bash.rs`: let the sibling see the guard

`BashCancelGuard` is currently private to `session::bash`, so `session::compaction` cannot import it
at all. Its twin `CompactionCancelGuard` is already `pub(super)` (`compaction.rs:436`). Bring it in
line — one word, still `session`-internal, nothing leaves the module tree:

```rust
pub(super) struct BashCancelGuard<'a> {
```

### Fix B — `src/session/compaction.rs`: resolve the guard cross-reference

`CompactionCancelGuard`'s doc says it plays *"the same role [`BashCancelGuard`] plays"* — a
deliberate cross-reference between the two guards, and worth keeping alive. Add beside the file's
existing imports:

```rust
// Doc-only: this guard's rationale is written against its twin in `bash.rs`, which nothing here
// names in code. Same `cfg(doc)` treatment as `types.rs`/`accessors.rs`/`mod.rs`.
#[cfg(doc)]
use super::bash::BashCancelGuard;
```

### Fix C — `src/session/mod.rs`: resolve the error-variant link

The `runtime_actions` field doc (line 292) names the variant a runtime-tier op surfaces. Add beside
the `#[cfg(doc)] use compaction::CompactionCancelGuard;` already there:

```rust
// Doc-only: the `runtime_actions` field doc names the variant a runtime-tier op surfaces;
// `session.rs:35` had this in scope, and nothing in `mod.rs` names it in code.
#[cfg(doc)]
use crate::error::SessionServiceError;
```

## Leave these three alone — pre-existing, not ours

Also surfaced by the private-items sweep, and **already broken before the split** — neither name was
in `session.rs`'s import block nor defined in it, so these links never resolved. They belong to
[`CARGO_DOC_WARNINGS.md`](CARGO_DOC_WARNINGS.md):

- `forking.rs:652` and `forking.rs:658` — `` [`SessionLayout`] ``, `` [`SessionLayout::new`] ``
- `model.rs:151` — `` [`AuthStore::reload`] ``

## Definition of done

```bash
# 1. THE NEW GATE — private-items sweep leaves only the 3 pre-existing links
cargo doc -p cyrup-session-svc --no-deps --document-private-items --message-format short 2>&1 \
  | grep 'src/session/' | grep -c 'unresolved link'                                         # 3
#    ...and they are exactly SessionLayout x2 + AuthStore, nothing else:
cargo doc -p cyrup-session-svc --no-deps --document-private-items --message-format short 2>&1 \
  | grep 'src/session/' | grep 'unresolved link'

# 2. the five gates already met must NOT move
cargo doc --workspace --no-deps --message-format short 2>&1 | grep -c 'src/session/.*warning'      # 8
cargo doc --workspace --no-deps --message-format short 2>&1 | grep 'src/session/' | grep -c 'unresolved link'  # 0
cargo doc --workspace --no-deps --message-format short 2>&1 | grep -c 'cyrup-session-svc/src/.*warning'        # 19
cargo check  -p cyrup-session-svc --all-targets --message-format short 2>&1 | grep -c 'src/session/'           # 0
cargo clippy -p cyrup-session-svc --all-targets --message-format short 2>&1 | grep -c 'src/session/'           # 0

# 3. nothing else moved
cargo test -p cyrup-session-svc      # 311 passed, 0 failed
```

`pub(super)` on `BashCancelGuard` must not change gate 2 — it is `session`-internal and the struct
is never re-exported, so the normal doc build neither documents it nor links to it.

## Do NOT

- Reword any doc comment carried over from `session.rs`. All three fixes add code, not prose.
- Add `#[allow(unused_imports)]` — `#[cfg(doc)]` makes it unnecessary.
- Touch the 6 `links to private item` warnings, the 3 pre-existing unresolved links, or anything
  outside `src/session/`.
- Move code between modules, widen any visibility beyond Fix A, or run `cargo fmt`.
