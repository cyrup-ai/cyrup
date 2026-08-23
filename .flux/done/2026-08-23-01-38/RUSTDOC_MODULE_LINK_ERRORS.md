---
stage: qa
status: completed
updated: 2026-08-23 02:26
---

# Rustdoc Link Errors — COMPLETED

**Resolved 2026-08-23 02:26.** `cargo doc --workspace --no-deps --bins` exits 0. Roughly 108 links across
51 files plus the binary target, over seven verification rounds.

Two things in this task were themselves wrong and were corrected by evidence rather than argument:
its prescribed path for `ModelThinkingLevel` named a crate the binary does not depend on (the
working path is `cyrup_sdk::core::…`, which `main.rs` imports three lines below that doc comment),
and its bracket-sweep produced 259 candidates that were all false positives — brackets inside
backticks or code fences are not link syntax.

Verified in the RENDERED HTML, not just by exit code: `argv[0]` prints as literal text with its
brackets intact, `ThemeData` resolves to `struct cyrup_resources::theme::ThemeData`, and
`ModelThinkingLevel` to `enum cyrup_core::message::thinking::ModelThinkingLevel`.

The gate was documented nowhere, so the `--bins` form was added to the README Build block beside
the other gates — a deny lint nobody is told to run cannot stop regrowth.

Final: doc gate exit 0; lints still `deny` on all three; zero `#[allow(rustdoc::…)]`;
`cargo check --workspace --all-targets` clean; `cargo nextest run --workspace` 7859/7859.

---

## The rework as filed — QA verdict at the time: 9/10

**The library work is done and done well.** `cargo doc --workspace --no-deps` exits 0 with zero
diagnostics, across roughly 105 links in six iterations. Spot-audited and confirmed correct:

- The canonical-path rule held: `crate::extension::SubagentExecutor`, never the private
  `::executor::` path that would also have resolved.
- The six renames are semantically right, verified against code rather than plausibility.
  `route_action` really does handle `"stop"` in its control arm ([routing.rs:744](../../crates/cyrup-ext-subagents/src/extension/tool/routing.rs)); the `catch_unwind` really is inside
  `Agent::start_run` ([lifecycle.rs:326](../../crates/cyrup-agent/src/agent/lifecycle.rs)); `timeout_ms` really is a field of
  `GenerationConfig` ([state.rs:83](../../crates/cyrup-agent/src/state.rs)).
- The eight flattenings each had no reachable target, and each reads naturally as prose.
- Constraints held: lints still `deny`, zero `#[allow(rustdoc::…)]`, no `use` added,
  100 insertions against 100 deletions — every edit a same-line retarget.
- `cargo check --workspace --all-targets` clean; `cargo nextest run --workspace` 7859/7859.

One thing is outstanding, and it is the verification command itself.

---

## 1 · The DoD's own gate does not cover the binary target

`cargo doc --workspace --no-deps` documents **lib targets only**. Adding `--bins` surfaces three
real errors the accepted gate never sees, all in
[crates/cyrup/src/main.rs](../../crates/cyrup/src/main.rs):

| line | link | fix |
|---|---|---|
| 57 | `[\`0\`]` — from the prose `argv[0]` | **escape the brackets**: `argv\\[0\\]` |
| 2154 | `[\`ThemeData\`]` | `cyrup_resources::ThemeData` — root re-export, [lib.rs:66](../../crates/cyrup-resources/src/lib.rs) |
| 2233 | `[\`ModelThinkingLevel\`]` | `cyrup_core::ModelThinkingLevel` — root re-export, [lib.rs:33](../../crates/cyrup-core/src/lib.rs) |

**Line 57 is a different defect class from everything fixed so far** and deserves naming: nothing
was renamed or moved. `argv[0]` in prose is being parsed as markdown link syntax, so rustdoc hunts
for an item called `0`. The fix is to escape the brackets, not to find a target — rustdoc's own
help text says exactly this ("to escape `[` and `]` characters, add '\\' before them"). Grep the
tree for other unescaped `[N]` and `[x]` sequences in doc comments while you are here; this one
only surfaced because a bin target was finally documented.

Also emitted, and **not** a defect to fix: a filename-collision warning between the `cyrup` bin and
`cyrup` lib targets writing the same `target/doc/cyrup/index.html`. That is
[cargo#6313](https://github.com/rust-lang/cargo/issues/6313), a known upstream bug, and it is a
warning about output layout rather than about this workspace's docs. Do not restructure anything to
silence it.

## 2 · Make the gate cover what it claims

The point of the `deny` lint is that broken links cannot accumulate again. A gate that skips the
binary leaves a hole exactly where a reader is most likely to start. Update the verification command
— everywhere it appears, including this task's own definition of done and any xtask or README
reference — to:

```sh
cargo doc --workspace --no-deps --bins
```

Check whether `xtask feature-matrix` or the README's Build section names the old form, and correct
those too, or the next person inherits the same blind spot.

## Definition of done

- [ ] `cargo doc --workspace --no-deps --bins` exits 0 with zero errors and zero warnings, except the known cargo#6313 filename-collision note
- [ ] `main.rs:57`'s `argv[0]` has escaped brackets rather than a link target
- [ ] `main.rs:2154` and `:2233` link the crate-root re-exports named above
- [ ] The tree has been grepped for other unescaped `[N]`-style sequences in doc comments
- [ ] Every place that documents the doc gate names the `--bins` form
- [ ] `[workspace.lints.rustdoc]` still `deny`; still zero `#[allow(rustdoc::…)]`; no `use` added
- [ ] `cargo check --workspace --all-targets` and `cargo nextest run --workspace` unchanged (7859/7859)

## Note

This is the second time this task's stated count has proved low — 33, then 88, now 88-plus-3-behind
-a-flag. Each undercount came from a verification command that stopped early: first a disk abort,
then cargo abandoning sibling jobs, now a target class the command never asked for. Worth carrying
forward: when a count comes from a tool, check what the tool was scoped to before trusting it.
