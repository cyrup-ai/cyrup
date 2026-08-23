---
stage: qa
status: completed
updated: 2026-08-22 21:09
---

# Decompose ops/local.rs Into Submodules — Rework

The decomposition itself is complete and verified (QA 9/10): twelve files under
`crates/cyrup-tools/src/ops/local/`, acyclic layering, byte-identical moved code, unchanged public
API, 16 relocated tests passing, zero clippy warnings in the new tree, rustdoc warnings down from
10 to 8, and `cargo check -p cyrup-tui` confirming the cross-crate re-export chain still resolves.
None of that needs revisiting.

Two items remain.

## 1. Three stale "the module doc comment" cross-references

The `exec` vs `exec_argv` escalation rationale used to live in `local.rs`'s module doc. It now
lives in `proc.rs`'s module doc. Three references still say "the module doc comment", which in
their new files resolves to a module doc that does **not** carry that content — each merely
redirects to `super::proc`. The reader gets there in two hops instead of one, and the pointer names
the wrong module.

`proc.rs:33` uses the same phrase and is correct (it is in `proc.rs`, which does carry the essay) —
leave that one alone. It is also what makes the other three read as errors rather than shorthand.

Fix, prose-minimal — change only the pointer, nothing else on the line:

- **`src/ops/local/signal.rs:52`**, in `send_sigkill_tree`'s doc:
  ``(no `SIGTERM`, no grace period, ever — see the module doc comment)``
  → ``… — see [`super::proc`]'s module doc comment)``
- **`src/ops/local/signal.rs:55`**, same doc block:
  `see the module doc comment for why the two methods diverge.`
  → ``see [`super::proc`]'s module doc comment for why the two methods diverge.``
- **`src/ops/local/command.rs:90`**, in `build_argv_command`'s body comment:
  `(see the doc comment above and the module doc comment)`
  → ``(see the doc comment above and `super::proc`'s module doc)``
  This one is a `//` comment, not a doc comment, so use a plain code span — a `[` `]` link in a
  non-doc comment is dead text.

Keep each edit inside its existing line where it fits. `signal.rs:52` and `command.rs:90` will
exceed 100 columns; rewrap only the lines you touched, and only within their own paragraph. Do not
re-flow the surrounding prose.

## 2. Do not run `cargo fmt`

The task's original procedure told you to. **It is wrong for this repo and must not be repeated.**
No crate in this workspace is rustfmt-clean at HEAD — `cyrup-core`, `cyrup-agent` and
`cyrup-session` all fail `cargo fmt --check`, and no config reproduces the house style (the closest,
`use_small_heuristics=Max,max_width=100`, still leaves 14 hunks in `cyrup-core`). Running
`cargo fmt` anywhere reformats that entire package: it previously rewrote 40 unrelated files in
`cyrup-tools` and had to be reverted.

`cargo fmt --check` is therefore **not** a gate on this task and must not be treated as one. The
relevant measurement instead: HEAD's single `local.rs` carried 37 rustfmt hunks, the new tree
carries 38, and all 38 sit in moved code with none in the new hand-written headers — i.e. the split
added no formatting debt. Keep it that way.

## Verification for the rework

```
cargo doc --no-deps -p cyrup-tools 2>&1 | grep -c '^warning'   # must stay at 8, no new unresolved links
cargo check -p cyrup-tools --all-targets                        # clean
```

Doc-comment text is the only thing changing, so the test suite does not need a re-run beyond the
check above. Do not run `cargo fmt`.
