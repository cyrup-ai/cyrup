---
stage: qa
status: completed
updated: 2026-08-27 15:55
severity: high
effort: small
subsystem: fork context / thinking level
source: docs/gap-analysis/09a-cyrup-ext-subagents-v0.57-drift.md
item: SUBA-075
---

> **Path note.** This task lives in a subdirectory. Every flux command globs a single level (`ls -1 "$FLUX_BASE/todo/"*.md`), so `/exec`, `/aug` and `/qa` will not list it — pass the absolute path explicitly.

# SUBA-075 rework — the fork thinking gate reads a persona's `model:` before the `inherit` sentinel is purged

The SUBA-075 port itself is complete and verified (all three fork outcomes, the doubly-gated
override, the byte-faithful rewrite, both `apply_thinking_suffix` call sites; 2533 tests green,
clippy clean, `cargo doc` clean). This file is the ONE outstanding item, plus the evidence needed to
land it in a single pass.

---

## Correction to the QA write-up

**The patch the QA pass prescribed does not compile.** It applied the filter after
`.map(ModelId::as_str)`:

```rust
        .map(ModelId::as_str)
        .filter(|model| real_requested_model(Some(model)).is_some())   // ✗
```

[`real_requested_model`](../../../crates/cyrup-ext-subagents/src/exec/fallback.rs) takes
`Option<&ModelId>`, not `Option<&str>` — at that point in the chain the item is already `&str`, so
the closure would be handing it `Option<&&str>`. The corrected form is below; it filters while the
items are still `&ModelId` and uses `filter_map`, whose closure receives `Self::Item` directly
rather than a reference to it (so no `*model` deref dance).

Two further facts the QA pass left as open questions, now settled:

- `real_requested_model` is **private** (`fn`, `fallback.rs:66`), used at three sites all inside
  `fallback.rs` (`:285`, `:295`, `:344`). It needs `pub(crate)`.
- The gate sits at `foreground.rs:371`; `resolve_model_inheritance` is called at `foreground.rs:466`.
  The gap is **95 lines**, confirming the QA pass's "~100 lines later".

---

## Why this is reachable — the evidence

Discovery does not normalize the sentinel. The `model` frontmatter field is a raw pass-through
([`discovery/frontmatter.rs:843`](../../../crates/cyrup-ext-subagents/src/discovery/frontmatter.rs)):

```rust
    let model = parsed.get("model").map(ModelId::from);
```

`grep -i inherit` across `src/discovery/` returns only `inheritSkills`/`inheritProjectContext` hits —
nothing touching `model`. So a persona whose frontmatter says `model: inherit` arrives at
`resolve_run_agent` as `AgentDefinition.model == Some(ModelId("inherit"))`, and the fork gate reads
it verbatim.

## Why this is a house invariant, not a nitpick

Every OTHER model-facing seam in this crate purges the sentinel, and both of them fall through to
the parent model — which is exactly the answer the fork gate is throwing away:

| seam | purge |
|---|---|
| launch path — [`exec/fallback.rs:285`](../../../crates/cyrup-ext-subagents/src/exec/fallback.rs) | `available_models.retain(\|m\| real_requested_model(Some(m)).is_some());` with a comment naming this exact hazard: *"a persona whose frontmatter says `model: inherit` would otherwise still be filtered in as candidate #0 and spawn a child with `--model inherit`"* |
| `models` report — [`extension/models/mod.rs:122`](../../../crates/cyrup-ext-subagents/src/extension/models/mod.rs) | `let explicit = (!trimmed.is_empty() && trimmed != INHERIT_MODEL_SENTINEL).then_some(trimmed);` then `None => parent_model.map(...)` |
| **fork thinking gate (new)** | **none** — this task |

Upstream never lets a sentinel reach the predicate either: `prepareForkThinking` resolves it through
`resolveEffectiveSubagentModel` *before* `buildModelCandidates`
(`subagent-executor.ts:5864-5879` @v0.57.0).

## Where the fix goes, and where it must NOT go

**It belongs at the ladder level, in `fork_requires_thinking_off`.** Two tempting alternatives are
both wrong, and rejecting them is part of this task:

1. **Not inside `forked_child_requires_thinking_off`.** Filtering there is a provable no-op: a
   filtered-to-`None` model hits `if model.is_none() → true`, and an unfiltered `"inherit"` hits
   `if info.is_none() → true`. Same answer. The sentinel only does damage because it sits in a
   *list* and short-circuits `.any` past the rungs that hold the real answer — so the filter has to
   happen where the list is built. It would also break that function's upstream contract, which is
   deliberately "unknown model → conservative true" with no sentinel awareness.
2. **Do not reorder `resolve_run_agent` to run the model ladder first and reuse
   `resolve_model_inheritance`'s already-purged `available_models`.** That was considered and
   rejected when this task was first designed: it changes which error surfaces first when a run has
   both a fork failure and a `ModelOutOfScope` violation, which is a behaviour change well outside
   this item.

**Equivalence to upstream.** Upstream resolves `inherit` → parent and puts that resolved primary
first; this filters `inherit` out and leaves the parent as an ordinary rung. The candidate SETS are
identical, and the consumer is `.any`, which is order-insensitive — so a full
`resolveEffectiveSubagentModel` port buys nothing here. Do not build one.

---

## Required change

### 1. `src/exec/fallback.rs` — widen the existing helper

```rust
pub(crate) fn real_requested_model(requested: Option<&ModelId>) -> Option<&ModelId> {
```

One word. Same change `extension::models` already took for `registry_models`. Do NOT re-declare the
predicate in `foreground.rs`: `real_requested_model` also trims and rejects blanks, and duplicating
that is how the two copies drift.

### 2. `src/extension/executor/foreground.rs` — purge the ladder

In `fork_requires_thinking_off` (`:888`), replace the candidate chain:

```rust
    let mut candidates = agent
        .model
        .iter()
        .chain(agent.fallback_models.iter())
        .chain(model_override)
        .chain(parent_model)
        .map(ModelId::as_str)
        .peekable();
```

with:

```rust
    // pi resolves the `inherit` sentinel through `resolveEffectiveSubagentModel` BEFORE building
    // candidates (`subagent-executor.ts:5864-5879` @v0.57.0), so it never reaches the predicate
    // upstream. Here it would: discovery hands `model: inherit` straight through as a `ModelId`
    // (`discovery/frontmatter.rs:843`), and `resolve_model_inheritance`'s own purge
    // (`exec/fallback.rs:285`) does not run until 95 lines further down `resolve_run_agent`. Left
    // in, an inheriting persona resolves to nothing, `forked_child_requires_thinking_off` takes its
    // conservative unknown-model arm, and `.any` short-circuits to `true` — past the `parent_model`
    // rung that holds the real answer.
    //
    // Filtering rather than resolving is faithful because the consumer is `.any`: upstream's
    // resolved primary IS the parent model, which is already a rung here, and `.any` does not care
    // which position it occupies.
    let mut candidates = agent
        .model
        .iter()
        .chain(agent.fallback_models.iter())
        .chain(model_override)
        .chain(parent_model)
        .filter_map(|model| crate::exec::fallback::real_requested_model(Some(model)))
        .map(ModelId::as_str)
        .peekable();
```

`filter_map`'s closure receives `&ModelId` (not `&&ModelId` as `filter`'s would), which is exactly
what `real_requested_model` takes, and it returns the `&ModelId` straight into `.map(ModelId::as_str)`.
Lifetime elision ties the output borrow to the input, so nothing needs annotating.

`parent_model` is inside the filter deliberately — it is harmless there (`remembered_parent_model`
already normalizes to a two-non-empty-halves `provider/id` via `normalize_parent_model`, so it can
never BE a sentinel) and keeping one uniform chain is simpler than splitting the map in two.

---

## Definition of done

The gate answers on the effective ladder rather than on the sentinel:

1. Persona `model: inherit`, no fallbacks, no override, **non-Anthropic** parent → `false`.
   This is the regression; it returns `true` today.
2. Persona `model: inherit` with an **Anthropic** parent → still `true`. The fix narrows the gate,
   it does not disarm it.
3. Persona `model: inherit`, no fallbacks, no override, **no parent** → `true`, now reached through
   the empty-ladder arm (`candidates.peek().is_none()`) for the right reason instead of through an
   unresolvable candidate.
4. The four existing gate behaviours are unchanged: an Anthropic-free ladder clears, one Anthropic
   rung anywhere forces off, an empty ladder forces off, an external runner forces off ahead of the
   ladder.
5. `cargo test -p cyrup-ext-subagents`, `cargo clippy -p cyrup-ext-subagents --all-targets` and
   `cargo doc -p cyrup-ext-subagents --no-deps --lib` stay exactly as clean as they are now — 2533
   passing, no new clippy finding, no doc warning. Removing the `filter_map` must make (1) fail.

---

## Settled — do NOT reopen

Checked against upstream at v0.57.0 during QA; recorded so this pass does not re-litigate them.

- **`redacted` stripped unconditionally.** Upstream's `redacted_thinking` arm is unconditional, and
  cyrup's Anthropic adapter decodes that wire type into `Content::Thinking { redacted: true }`, so
  this is the faithful mapping. Documented in place as a `[CYRUP-DELTA]`.
- **`pub fn forked_child_requires_thinking_off`.** `fork_context.rs`'s convention is `pub` for its
  cross-module surface (`resolve_effective_context`, `ForkContext`, `ForkContextResolver`) and
  private for helpers; this one is called from `foreground.rs` and follows it.
- **`background.rs` / `plan_batch` / `slash_render` passing `true`.** Upstream's own
  `forceThinkingOffForIndex?.(index) ?? true`. It is also more than a fallback: forcing the append
  puts a `thinking_level_change` entry in the branch, and `SessionManager::build_context`
  (`cyrup-session/src/manager/context.rs:28`) folds those last-wins into
  `SessionContext::thinking_level` — so a resuming child picks the level up from the transcript even
  though `SingleStepSpec` cannot carry the argv override across hop 2.
- **Non-atomic `write_session_entries`.** Matches upstream's `fs.writeFileSync`; the branch file is
  a derived artifact regenerable from an untouched parent.
- **Upstream's `record.signature` fallback key is not read.** `cyrup_core::Content::Thinking` models
  only `thinkingSignature`, so a foreign `signature` key is dropped at deserialize. Reading it would
  mean abandoning the typed `Entry` pass or changing `cyrup-core` — its own item if a legacy-pi
  import path ever needs it.
