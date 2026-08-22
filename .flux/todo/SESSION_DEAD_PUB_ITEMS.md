---
stage: new
status: done
updated: 2026-08-22 23:52
---

# Remove 5 dead public items in cyrup-session: TokenCache::estimate_entry + invalidate (which are the only reason EstimateKind exists), ContextStore::from_snapshot, SessionError::NotPersisted, and the never-called append_branch_summary twin

> Found by a six-lens hygiene audit of `crates/cyrup-session`, run after the `manager/`
> decomposition landed in PR #53. Every claim below was reproduced against the tree.
> **Priority:** medium · **Effort:** small

Five `pub` items have no production consumer. Because they are `pub`, `dead_code` is suppressed, which is why `cargo clippy --all-targets -p cyrup-session` reports 0 findings today.

## 1. `TokenCache::estimate_entry` + `invalidate` — and the `EstimateKind` discriminator they exist for

`compaction/tokens.rs:262` (`estimate_entry`) and `:292` (`invalidate`) have **zero call sites anywhere in the workspace** — not in src/, not in cyrup-session's own 157 tests, not in any sibling crate. The only live cache method is `estimate_raw_entry` (`:282`).

Consequences:
- `EstimateKind::Rendered` (`:231`) is reachable *only* from these two dead methods, so the whole two-variant `EstimateKind` enum and the `(EntryId, EstimateKind)` composite HashMap key (`:243`) exist purely to serve dead code. The map can be keyed on `EntryId` alone.
- `use crate::context::push_as_message;` (`tokens.rs:11`) becomes unused once `estimate_entry` goes — `:265` is its only call.
- The code contradicts itself: the `TokenCache` doc at `tokens.rs:239-240` states "Entries are immutable once appended => estimates never invalidate", directly above a public `invalidate` whose own doc says "only needed on rare entry mutation".

Removing both + collapsing `EstimateKind` deletes ~20 lines from a hot compaction path and a misleading extension point.

## 2. `ContextStore::from_snapshot` (`prompt/cache.rs:58`)

`grep -rnw from_snapshot crates/ xtask/ --include='*.rs'` returns **exactly one line — its own definition**. Its doc claims it exists "e.g. in tests / restore", which is false in the current tree.

## 3. `SessionError::NotPersisted` (`error.rs:31`)

Declared `#[error("operation requires a persisted session")]`; a workspace-wide grep returns **one hit, the definition**. Nothing ever constructs it. Meanwhile the downstream crate that needs this condition invented its own: `cyrup-ext-subagents/src/error.rs:27-28` `ForkRequiresPersistedParent`. Either delete the dead variant from the public `SessionError`, or make the ephemeral/`MemStore` paths return it so both crates share one vocabulary.

## 4. `SessionManager::append_branch_summary` (`manager/navigate.rs:58`)

A near-duplicate of `branch_with_summary` (`navigate.rs:31`) sitting 20 lines away. Both build the same `KnownEntry::BranchSummary` with the same six fields (`base`, `from_id`, `summary`, `details`, `usage`, `from_hook`); the only difference is that `append_branch_summary` does not move the leaf.

- `branch_with_summary` is the live path: `compaction/mod.rs:422` and `cyrup-session-svc/src/session/forking.rs:295`.
- `append_branch_summary` has **zero production callers**; its only three call sites are cyrup-session's own tests: `tests/sessions.rs:674`, `tests/compaction.rs:1190`, `tests/compaction.rs:1656`.
- Its doc (`navigate.rs:55`) claims it implements "the corrected R-05-016" — so the tree currently documents the dead function as the correct behaviour while shipping the other one.

Resolve which is canonical: delete the dead one, or migrate the two production callers to it and delete `branch_with_summary`. Whichever survives, fix the R-05-016 doc so it describes shipped behaviour.

## Also consider (test-only `pub`, cheaper judgement calls)

These are defined in src/ but called only from cyrup-session's own test modules — make them `pub(crate)`, `#[cfg(test)]`, or wire them: `ContextFileLoader::from_trust` (`prompt/context_files.rs:94`) plus the `TrustQuery` trait (`:46`, only impl is a test `Stub` at `prompt/tests.rs:46`; production builds the loader with `ContextFileLoader::new` at `cyrup-session-svc/src/builder.rs:1243`); `ResolvedOverride::join_appends` (`prompt/overrides.rs:37`); `SessionManager::leaf_entry` (`manager/tree.rs:29`); `Entry::type_tag` (`entry.rs:271`); `SystemPromptBuilder::inputs_fingerprint` (`prompt/builder.rs:262`, which already documents itself as dead at `:251`).

## Acceptance Criteria

- [ ] `grep -rnw 'estimate_entry\|from_snapshot\|NotPersisted' crates/ xtask/ --include='*.rs'` returns 0 hits, or each remaining item has a non-test caller
- [ ] `TokenCache::invalidate` is gone and the cache map is keyed on `EntryId` alone (`grep -n EstimateKind crates/cyrup-session/src/compaction/tokens.rs` returns nothing), with `use crate::context::push_as_message;` removed from tokens.rs if now unused
- [ ] Exactly one of `branch_with_summary` / `append_branch_summary` remains in `manager/navigate.rs`, both production call sites (`compaction/mod.rs:422`, `cyrup-session-svc/src/session/forking.rs:295`) use it, and its doc's R-05-016 claim matches shipped behaviour
- [ ] The TokenCache doc no longer contradicts the API it documents (no `invalidate` alongside "estimates never invalidate")
- [ ] `cargo test -p cyrup-session` passes with tests updated, not deleted, where they covered surviving behaviour
- [ ] `cargo clippy --all-targets -p cyrup-session -p cyrup-session-svc` reports 0 findings

## Verifying command

```bash
cd /home/user/cyrup && grep -rnw 'estimate_entry\|from_snapshot\|NotPersisted' crates/ xtask/ --include='*.rs'; grep -rn 'EstimateKind\|fn invalidate' crates/cyrup-session/src/compaction/tokens.rs; grep -rn 'append_branch_summary\|branch_with_summary' crates/ --include='*.rs'
```
