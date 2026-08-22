---
stage: qa
status: needs-rework
updated: 2026-08-22 16:53
---

# Decompose cyrup-session-svc src/session.rs Into Submodules — Outstanding Work

## QA verdict: 8/10 — needs rework

The decomposition itself is complete and correct. `src/session.rs` (6297 lines) is now
`src/session/mod.rs` + 20 concern submodules (largest 696 lines), every build gate is green, and
all 311 existing tests pass. **The structural work below is verified done and is not to be redone.**

One regression blocks a 10: the split **introduced 7 new `cargo doc` warnings**. A refactor billed
as behaviour- and quality-neutral must not leave the codebase measurably worse on any lint axis.

### Verified complete — do NOT redo

- 21 files under `src/session/`; `src/session.rs` removed; no file over ~700 lines (max 696).
- `cargo check -p cyrup-session-svc` — `--lib`, `--lib --no-default-features`, and `--all-targets`
  all pass with **0 errors and 0 warnings** under `src/session/`.
- `cargo clippy -p cyrup-session-svc --all-targets` — **0 warnings** under `src/session/`. The 4
  remaining crate warnings are pre-existing, in `src/bash.rs`, `src/tests/round9_l5res.rs` and
  `src/tests/fork_parent_and_unsaved_guard.rs` — files this change never touched.
- `cargo test -p cyrup-session-svc` — **311 passed, 0 failed**.
- `cargo check --workspace` passes; `src/lib.rs` is untouched and its `pub use session::{…}` list
  resolves unchanged.
- Line-by-line audit against the original confirms every absent line is either a regrouped `use`
  continuation or a signature that gained `pub(super)`. No doc comment, body or banner reference
  was lost.
- 20 `pub(super)` widenings, all `session`-internal. Dropping the 4 the plan listed for items that
  stay in `mod.rs` was **correct** — Rust already grants descendants access there, and adding
  `pub(super)` would have widened visibility beyond the original.
- `src/tests/compaction_tokens_after.rs` repaired faithfully (its `include_str!("../session.rs")`
  now reads the two compaction modules its own doc names).

---

## 1. Fix the 7 intra-doc links the split broke

Every one of these names was in scope inside the single `session.rs` and is not in scope in the
module its doc landed in. All 7 are **newly introduced** by this change:

| File:line | Unresolved link |
| --- | --- |
| `session/accessors.rs:258` | `ProviderSwap` |
| `session/types.rs:72` | `AgentSession::session_dag` |
| `session/types.rs:112` | `AgentSession::navigate_tree` |
| `session/types.rs:132` | `AgentSession::cycle_model` |
| `session/types.rs:141` | `AgentSession::bind_extensions_with` |
| `session/mod.rs:555` | `CompactionCancelGuard` |
| `session/mod.rs:564` | `CompactionCancelGuard` |

**Do not reword the doc comments** — that constraint still stands. Fix by bringing the name into
scope. None of the three names appears in *code* in the file that needs it (verified: 0 code
occurrences each), so a plain `use` would be an unused import and trade a doc warning for a
`cargo check` warning. Use the doc-only import idiom instead:

```rust
// `session/types.rs` — these types' docs link to the seam that owns them; the import is
// `cfg(doc)` because nothing here names `AgentSession` in code.
#[cfg(doc)]
use super::AgentSession;
```

```rust
// `session/accessors.rs`
#[cfg(doc)]
use crate::provider_swap::ProviderSwap;
```

```rust
// `session/mod.rs` — the `Drop` rationale points at the guard that plays the same role.
#[cfg(doc)]
use compaction::CompactionCancelGuard;
```

Verify **both** directions afterwards — `#[cfg(doc)]` must not introduce an unused-import warning
under `cargo doc`, and must stay invisible to a normal build:

```bash
cargo doc   -p cyrup-session-svc --no-deps --message-format short 2>&1 | grep 'src/session/'
cargo check -p cyrup-session-svc --all-targets --message-format short 2>&1 | grep 'src/session/'
```

**Done when:** the first command emits only the 6 `links to private item` warnings listed below,
and the second emits nothing.

### Leave these 6 alone — they are pre-existing

`links to private item` is visibility-based and fires identically whether the code sits in one file
or twenty. All 6 predate this change and belong to the queued `CARGO_DOC_WARNINGS.md`:
`accessors.rs:145` (`Self::push_active_tools`), `bash.rs:165` (`Self::emit_user_bash_event`),
`control.rs:83` (`Self::apply_agent_state_op`), `model.rs:177` (`Self::full_model_registry`),
`queue.rs:107` (`ABORT_SETTLE_TIMEOUT`), `transcript.rs:126`
(`crate::host_services::tree_node_to_json`). The same crate carries 11 more of them in untouched
files (`host_services.rs`, `runtime.rs`, `event.rs`, `builder.rs`), which is what confirms the lint
was already active and already firing.

## 2. Tidy the two module docs that link out of a private module

`session/model.rs` links `[`ProviderSwap`]` and `session/mod.rs` links `[`run`]`/`[`commands`]`/…
from module-level docs. These do **not** warn today only because `mod session;` and its children are
private, so rustdoc never link-checks them. They are latent: the moment a module is made public the
links break. Either bring the names into scope the same way as item 1, or drop the brackets. This is
a tidy-up on docs **this change authored** — the no-rewording rule does not cover them.

## Note — not actionable, recorded so it is not re-litigated

The original plan's Step 0 (`git mv` "so history follows") and its final bullet (`git diff -M --stat`
shows the change as renames + moves) **cannot hold for a 1→21 split** and were correctly reported as
unmet. Git detects renames by content similarity at diff time, needing one successor ≥50% similar;
the largest piece here is 11% of the original, so no rename pair exists to detect and `git mv` would
have produced byte-identical results. `git log --follow` and `git blame` will stop at this commit for
the new files. That is inherent to splitting a file, not a defect in the execution.

Also recorded: the exec report described this doc-link fallout as "two intra-doc links". The measured
number is **7**. The count is what item 1 above is scoped against.
