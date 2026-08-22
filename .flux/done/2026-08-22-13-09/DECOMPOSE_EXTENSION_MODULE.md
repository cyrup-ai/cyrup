---
stage: qa
status: completed
updated: 2026-08-22 18:50
---

# Decompose cyrup-ext-subagents extension.rs — QA Rework (round 2)

## QA verdict: 9/10

The previous rework executed all three items correctly. Verified independently this round:

- **The rewrapped prose lost nothing.** `extension/mod.rs`'s module doc, unwrapped and compared
  word-for-word against the pre-split original, is identical modulo exactly the six specified
  substitutions plus the new paragraph. All three `testsupport.rs` paragraphs pass the same check.
- **All 23 import replacements are present verbatim**, no aliases introduced, zero `use` lines over
  100 characters.
- **The C7 block** is gone from `executor/requests.rs` (which still opens `StatusViewSelector`
  cleanly) and sits directly above `default_async_root` in `executor/paths.rs`.
- Every 2d do-not-touch site is intact.
- `cargo check --workspace --all-targets` clean · 2,484 tests pass · clippy reports exactly the two
  pre-existing findings · rustdoc warnings under `extension/` at **26**, the pre-rework baseline.
- The deviation from the task's literal text (inline code instead of intra-doc links in the seam
  paragraph) was correct: the links would have added five `private_intra_doc_links` warnings, which
  the task's own acceptance criterion forbids.

**One new defect was introduced, in the paragraph the rework added.** That is the whole of what
stands between this and 10/10.

---

## Item 1 — a false claim in the new seam paragraph (must fix)

[`extension/mod.rs:28-29`](../../crates/cyrup-ext-subagents/src/extension/mod.rs) now reads:

```rust
//! dispatch table is `tool::routing`, and the one sanctioned `cyrup-session` dependency is
//! `executor::resolve`'s `fork_resolver`.
```

That is **not true**, and it contradicts the paragraph directly below it, which describes a
throwaway `SessionManager` opened fresh per dispatch call. Production (non-test, non-doc)
`cyrup_session::` use is spread across three modules, counted this round:

| Module | Production uses | What they are |
|---|---|---|
| [`executor/resolve.rs`](../../crates/cyrup-ext-subagents/src/extension/executor/resolve.rs) | 7 | `fork_resolver`, `SessionLayout::new`, `open_with_cwd`, `continue_recent`, `in_memory` |
| [`executor/reports.rs`](../../crates/cyrup-ext-subagents/src/extension/executor/reports.rs) | 4 | `run_doctor` / `run_cost_report` open `SessionManager` directly |
| [`executor/paths.rs`](../../crates/cyrup-ext-subagents/src/extension/executor/paths.rs) | 3 | `unreachable_session_manager` constructs one |

The original pre-split sentence this was summarising made a **narrower** claim — that there is no
extension-host session-access *seam* beyond the sanctioned `crate::fork_context` dependency — and
that narrower claim is still true and still present, unchanged, at lines 22–23. The new sentence
widened it into a false statement about the whole crate's `cyrup-session` usage.

**Fix:** scope the clause back to the fork-context seam it was actually about. Replace lines 28–29:

```rust
//! dispatch table is `tool::routing`, and the fork-context seam named above is
//! `executor::resolve`'s `fork_resolver`.
```

Do not restate a "one dependency" claim in any form — `run_doctor`, `run_cost_report` and
`unreachable_session_manager` each reach `cyrup-session` directly, and any summary that implies
otherwise is wrong the moment it is written.

The paragraph's other two claims were checked this round and are **correct** — leave them alone:
`drive_foreground_run_sync` really is the sole `crate::exec::run_sync` call site (both occurrences
are inside it), and `executor/background.rs:517` really is the only `spawn_detached_runner` call.

## Item 2 — two over-length doc lines in the same file

Reported and consciously deferred last round as out of scope. `extension/mod.rs` is being edited
again for Item 1, so close them out now rather than carrying them a third cycle. Both were
lengthened by the split's own intra-doc-link expansion and never rewrapped:

- **Line 9 (114 chars)** — `//! into the one [`cyrup_ext::native::NativeExtension`] the `cyrup` binary registers (`crates/cyrup/src/main.rs`'s`
  Rewrap the paragraph it belongs to (lines 5–10).
- **Line 41 (132 chars)** — `//! `SessionManager` handle into [`cyrup_ext::native::InitApi`]/[`cyrup_ext::native::HostCtx`] at construction or dispatch time, and`
  Rewrap the paragraph it belongs to (lines 36–45).

Rewrap only — do not reword, and do not shorten the link paths back down: the fully-qualified forms
are what makes them resolve from this module, and shortening them would reintroduce the unresolved
links the split's doc-link pass fixed. Keep every line at or under 100 characters, and re-run
`cargo doc` afterwards to confirm the `extension/` warning count is still 26.

## Item 3 — decision recorded, no action

`tool/text.rs:660`'s `use crate::registration::tool_description::{…}` brace group does not follow
the member-ordering rule (`ToolDescriptionOptions` sits second, between `build_subagent_tool_
description` and the SCREAMING_CASE names). **Leave it.** It is byte-identical to the pre-split
original at `extension.rs:20752-20756` — hand-written by the crate authors, not generated by the
split — and its existence is itself evidence the repo does not enforce the rule universally
(the crate has 2,863 rustfmt diffs overall). Reordering it would be unrequested churn in someone
else's code. This is recorded so it is not re-raised on the next cycle.

## Acceptance criteria

- [ ] `extension/mod.rs` makes no claim about a single `cyrup-session` dependency; the seam
      paragraph's third clause is scoped to the fork-context seam.
- [ ] No line in `extension/mod.rs` exceeds 100 characters.
- [ ] The two verified-correct claims in the seam paragraph are unchanged, and the pre-split
      sentence at lines 22–23 is unchanged.
- [ ] `cargo check --workspace --all-targets` clean, `cargo test -p cyrup-ext-subagents` still
      2,484 passing, `cargo doc -p cyrup-ext-subagents --no-deps` still 26 warnings under
      `extension/`, and clippy still exactly the two pre-existing findings.
- [ ] No file outside `crates/cyrup-ext-subagents/src/extension/mod.rs` is modified.
