---
stage: qa
status: completed
updated: 2026-08-28 21:10
severity: high
effort: small
subsystem: fork context / launch policy
source: docs/gap-analysis/09a-cyrup-ext-subagents-v0.57-drift.md
item: SUBA-079
---

> **Path note.** This task lives in a subdirectory. Every flux command globs a single level (`ls -1 "$FLUX_BASE/todo/"*.md`), so `/exec`, `/aug` and `/qa` will not list it — pass the absolute path explicitly.

# SUBA-079 — QA rework: a redundant, shadowing `config_snapshot()` in `spawn_background`

**QA verdict 9/10.** All three sub-features are correctly implemented and the design decisions hold
up under audit — see the settled list below. 2569 tests pass, clippy is back to its 5 pre-existing
warnings, `cargo doc` is clean, and the workspace check is clean. This is a HYGIENE item, not a
correctness one: do not over-engineer it.

---

## The defect

[`extension/executor/background.rs`](../../../crates/cyrup-ext-subagents/src/extension/executor/background.rs)
now takes **two** config snapshots inside the single function `spawn_background`:

```rust
        let cfg = self.config_snapshot().await;          // :78 — pre-existing
        let depth = resolve_effective_depth(cfg.max_subagent_depth);
        …
        // SUBA-079: same three rungs as the foreground path …
        let cfg = self.config_snapshot().await;          // :107 — added by this task, SHADOWS :78
```

Nothing between them moves or invalidates the first binding — its only use is
`cfg.max_subagent_depth`, a `Copy` field read — so the second is purely redundant. It costs an extra
lock-and-clone on every background launch, and it silently shadows a live binding, so a reader at
`:117` cannot tell which `cfg` they are looking at.

The sibling path got this right: `foreground.rs` has exactly ONE `config_snapshot()` (`:236`) and
this task correctly reused it for `cfg.default_subagent_context`. `slash_render` likewise reuses the
`ForkContextResolver` it already holds instead of opening a second session. So this is an internal
inconsistency within the change, not a considered trade-off.

## Required change

Delete the added `let cfg = self.config_snapshot().await;` at `background.rs:107`. The binding from
`:78` is in scope and valid at `:117`. Keep the SUBA-079 comment above it — the comment is about the
ladder, not the snapshot.

## Definition of done

1. `spawn_background` performs exactly one `config_snapshot().await`.
2. No behaviour change: the `default_subagent_context` rung still reaches
   `resolve_effective_context`, and every SUBA-079 test still passes.
3. `cargo test -p cyrup-ext-subagents`, `cargo clippy -p cyrup-ext-subagents --all-targets` and
   `cargo doc -p cyrup-ext-subagents --no-deps --lib` stay as clean as they are now (2569 passing,
   no new clippy finding, no doc warning).

---

## Settled — do NOT reopen

Audited this pass; recorded so the next round is one round.

- **The blanket find-and-replace landed correctly.** Every `ContextRequest::` site in the tree is a
  REQUEST position (request-struct literals, `run_foreground` arguments, `context_override`'s own
  arms and tests, the `apply_fork_contexts` call-site argument, and the `.map(ContextRequest::from)`
  conversions). Every RESOLVED position kept `ContextMode` — `ForkContext::mode`,
  `AgentDefinition::default_context`, and the per-step `spec.context`. The two types are not
  interchangeable, so a wrong replacement is a hard error rather than a silent semantic change.
- **The policy is a faithful port** of `resolveSubagentLaunchContext` + the `profile` branch:
  explicit `Fresh`/`Fork` returned verbatim and strict; `profile` taking the agent's declaration and
  ignoring both the config rung and the availability test; then
  `config_default.or(agent_default).unwrap_or_default()` with the `Fork && can_prefer_fork`
  downgrade. The config rung outranks the agent's, as upstream has it.
- **All three call sites gate correctly**: `can_prefer_fork` is consulted only when nothing explicit
  was named, and `false` is passed otherwise — safe, because the policy never reads it on any
  explicit arm. Each threads and validates the config rung.
- **`context_override` matches each value exactly**, with an unrecognized value falling through to
  the defaults ladder rather than collapsing to an explicit `fresh`.
- **Upstream's `if (agent && …)` skip for an UNRESOLVED agent name is unreachable here**, so its
  absence is not a divergence: `resolve_plan_personas` (`chain.rs:330`) and
  `resolve_agent_with_model_scope` both reject an unknown agent BEFORE the policy runs.
- **`ContextMode` still has exactly two variants**, so no arm was added to `context_str`, the TUI
  badge, or the frontmatter writer.
- **The module doc was correctly rescoped** — the fail-hard rule now reads as governing an EXPLICIT
  `context: "fork"`, with the inherited preference documented as downgrading.
- **The schema enum and description match upstream**, and the pinning test was updated.
- **Seven mutations, all caught**, including the two traps this item is most likely to regress on:
  agent-outranks-config, and `context_override` collapsing again.
