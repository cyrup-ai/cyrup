---
stage: done
status: completed
updated: 2026-08-28
---

# Scope `/scoped-models` Enable-All And Clear-All To The Active Search Filter

> Identified by the `cyrup-tui` <-> pi port audit (fan-out + adversarial verification).
> **Priority:** medium · **Effort:** small · Area: Selectors, settings and dialogs

## Objective

In `/scoped-models`, typing a query to narrow the list and then pressing enable-all should add only
the matching models, and clear-all should remove only the matching models — pi's behaviour. Today
both keys ignore the query: enable-all turns on the entire catalog, and clear-all wipes the whole
scoped set, destroying a hand-built ordering the user cannot get back.

## Upstream reference

[`scoped-models-selector.ts:333-351`](../../tmp/pi/packages/coding-agent/src/modes/interactive/components/scoped-models-selector.ts)
computes the same target set for **both** arms:

```ts
const targetIds = this.searchInput.getValue() ? this.filteredItems.map((i) => i.fullId) : undefined;
this.enabledIds = enableAll(this.enabledIds, this.allIds, targetIds);   // :335-343
// …
this.enabledIds = clearAll(this.enabledIds, this.allIds, targetIds);    // :345-353
```

The helpers honour the targets (`:32-47`):

```ts
function enableAll(enabledIds, allIds, targetIds) {
  if (enabledIds === null) return null;                     // already all-enabled
  const targets = targetIds ?? allIds;
  const result = [...enabledIds];
  for (const id of targets) if (!result.includes(id)) result.push(id);
  return result.length === allIds.length && result.every((id) => allIds.includes(id)) ? null : result;
}

function clearAll(enabledIds, allIds, targetIds) {
  if (enabledIds === null) return targetIds ? allIds.filter((id) => !targetIds.includes(id)) : [];
  const targets = new Set(targetIds ?? enabledIds);
  return enabledIds.filter((id) => !targets.has(id));
}
```

Two details that are easy to lose: `enableAll` **preserves the existing order** and appends only ids
not already present; and `clearAll` from the all-enabled (`null`) state with a filter returns *every
catalog id minus the targets*, not an empty list.

## Current state in cyrup-tui

[`selector/checkbox.rs:506-513`](../../crates/cyrup-tui/src/selector/checkbox.rs):

```rust
ModelsAction::EnableAll => {
    self.enabled = None;
    self.dirty = true;
}
ModelsAction::ClearAll => {
    self.enabled = Some(Vec::new());
    self.dirty = true;
}
```

Neither arm consults `self.query` (the field is at `:90-98`, with the accessor `query()` at `:96`)
nor the filtered view `self.items()` (`:183-215`, which already runs `crate::fuzzy::filter` over the
sorted ids and returns `ModelItem { full_id, model, enabled }`). Both are used by the neighbouring
`ModelsAction::ToggleProvider` arm (`:514`) via `toggle_provider` (`:262-285`), which **already
contains the exact materialize / add / remove / collapse-to-`None` shape** this task needs:

```rust
let mut list: Vec<String> = match &self.enabled {
    None => self.rows.iter().map(|r| r.id.clone()).collect(),
    Some(l) => l.clone(),
};
// … add or retain …
self.enabled = if list.len() == self.rows.len() { None } else { Some(list) };
```

The **unfiltered** case is already correct (`None` and the empty vec are the right answers when
targets == allIds), so this is specifically the filtered branch. `src/tests/scoped_models.rs`
exercises the Ctrl+S save paths (`:264-300`, `:378-387`) but nothing asserts bulk-with-search.

## Subtasks

1. In [`selector/checkbox.rs`](../../crates/cyrup-tui/src/selector/checkbox.rs), add a private helper
   that returns the current target set: `None` when `self.query.is_empty()`, else
   `Some(self.items().into_iter().map(|i| i.full_id).collect())`.
2. Rewrite the `ModelsAction::EnableAll` arm (`:506-509`) to pi's `enableAll`: return early when
   `self.enabled` is already `None`; otherwise materialize the current list, append each target not
   already present (order preserved), and collapse to `None` only when the result covers every
   catalog id.
3. Rewrite the `ModelsAction::ClearAll` arm (`:510-513`) to pi's `clearAll`: from `Some(list)`,
   retain the entries not in the target set; from `None` **with** a target set, produce every catalog
   id minus the targets; from `None` with no target set, produce the empty vec (today's behaviour).
4. Reuse `toggle_provider`'s materialize/collapse block rather than duplicating it a third time —
   factor it out if that reads cleaner.

## Acceptance criteria

- [ ] `grep -n 'EnableAll\|ClearAll' -A6 crates/cyrup-tui/src/selector/checkbox.rs` shows both arms
      consulting the query / filtered items
- [ ] With an empty query, enable-all still yields `enabled == None` and clear-all still yields
      `enabled == Some(vec![])` — unchanged from today
- [ ] With a query matching a strict subset, enable-all from a partial set yields the previous list
      **plus** the matching ids, in the previous order, with matches already present not duplicated
- [ ] With a query, enable-all collapses to `None` only when the resulting list covers every catalog
      id
- [ ] With a query, clear-all from the all-enabled `None` state yields `Some(all_ids_minus_matches)`,
      not `Some(vec![])`
- [ ] With a query, clear-all from a partial set removes only the matching ids and leaves the rest in
      their existing order
- [ ] `cargo build -p cyrup-tui --all-targets` — 0 errors, 0 warnings
- [ ] `cargo clippy -p cyrup-tui --all-targets` — warning count not increased
- [ ] `cargo test -p cyrup-tui` — `src/tests/scoped_models.rs` still passes unchanged

## Constraints

- No tests are to be written for this task; another team owns the test suite.
- No benchmarks are to be written for this task.
- Workspace lints deny unwrap_used, expect_used, panic and indexing_slicing; cyrup-tui also has
  forbid(unsafe_code) and deny(clippy::string_slice).
- Mechanical fidelity to pi where pi's behaviour is the spec; Rust idiom where it is not.
