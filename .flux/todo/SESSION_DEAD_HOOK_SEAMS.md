---
stage: new
status: done
updated: 2026-08-22 23:52
---

# Delete or wire the two unwired hook seams in cyrup-session (352 lines): prompt/hook.rs and compaction::CompactionHooks both duplicate the live cyrup-ext dispatch

> Found by a six-lens hygiene audit of `crates/cyrup-session`, run after the `manager/`
> decomposition landed in PR #53. Every claim below was reproduced against the tree.
> **Priority:** high · **Effort:** large

cyrup-session carries two complete hook abstractions that no production code path ever reaches. Both duplicate a vocabulary that cyrup-ext already owns and wires, so the workspace currently has two `BeforeAgentStart*` types and two before-compact seams.

## Seam 1 — `prompt/hook.rs` (97 lines, 4 exported symbols, 0 external refs)

- `BeforeAgentStartInput` (`prompt/hook.rs:23`), `BeforeAgentStartOutput` (`:60`), `BeforeAgentStartHook` (`:79`), `apply_before_agent_start` (`:85`).
- Re-exported **twice**: `prompt/mod.rs:35` and `lib.rs:52`.
- `grep -rn 'apply_before_agent_start\|BeforeAgentStartHook' crates/ --include='*.rs' | grep -v crates/cyrup-session/` returns **0 lines**.
- Every implementor and caller lives in `src/prompt/tests.rs`: `ReplaceHook`/`AppendHook`/`KeepHook` at `:552`/`:560`/`:566`, calls at `:578`/`:582`/`:586`.
- The live seam of the same name is `cyrup_ext::ExtensionHost::emit_before_agent_start` (`crates/cyrup-ext/src/facade.rs:833`), with its own `BeforeAgentStartCombinedResult` (`facade.rs:28`).

## Seam 2 — `compaction/hooks.rs` (255 lines) + the `H: CompactionHooks` generic

- The only non-test `impl CompactionHooks` in the workspace is the null `NoHooks` (`compaction/hooks.rs:238`).
- All three production `Compactor::new` sites pass `NoHooks`: `cyrup-session-svc/src/session/forking.rs:69`, `.../auto_compaction.rs:210`, `.../compaction.rs:92`.
- `CompactionError::Hook(String)` (`compaction/error.rs:17-19`, doc: "Hook dispatch faulted") is constructed **0 times** anywhere: `grep -rn 'CompactionError::Hook' crates/ --include='*.rs' | wc -l` → `0`.
- The 6 hook-only event types re-exported from `compaction/mod.rs:36-39` all have **0 references outside `crates/cyrup-session/src`**: `BeforeCompactEvent`, `BeforeTreeEvent`, `BeforeTreeDecision`, `BeforeTreeOverrides`, `PostCompactEvent`, `PostTreeEvent`. (`BeforeCompactDecision` has exactly 1 external hit and it is a comment line in `cyrup-session-svc/src/session/compaction.rs:294`.)
- Cost of keeping it: a type parameter on `Compactor` threaded through every compaction entry point (`compaction/mod.rs:57` `pub struct Compactor<S: Summarizer, H: CompactionHooks>`, `:64` the impl block), plus 3 test hook impls in `src/tests/compaction.rs:163`, `:1118`, `:2674`.
- The extension dispatch that actually runs during compaction/forking goes through `cyrup-ext`'s `ExtensionHost`, not this trait.

## Decision required

For each seam, either (a) wire the cyrup-ext bridge through it and add a production caller, or (b) delete the trait, its event types, `CompactionError::Hook`, and drop the `H` generic from `Compactor`. Option (b) is the default unless someone can name the consumer. Deleting seam 2 collapses `Compactor<S, H>` to `Compactor<S>` and removes `NoHooks` from all three svc call sites.

Note: the 3 test hook impls in `src/tests/compaction.rs` exercise only the dead seam — they are deleted along with it, which also removes ~9 of the 46 tests in that file. Coordinate with TEST_FILE_DECOMPOSITION if both are in flight.

## Acceptance Criteria

- [ ] `grep -rn 'apply_before_agent_start\|BeforeAgentStartHook\|BeforeAgentStartInput\|BeforeAgentStartOutput' crates/ --include='*.rs'` either returns 0 hits (deleted) or includes at least one non-test production call site outside `crates/cyrup-session/src/prompt/tests.rs`
- [ ] `grep -rn 'CompactionError::Hook' crates/ --include='*.rs'` either returns 0 hits including the definition (variant deleted), or shows at least one construction site
- [ ] Either `compaction/mod.rs` no longer declares a `CompactionHooks` type parameter on `Compactor` (`grep -n 'H: CompactionHooks' crates/cyrup-session/src/compaction/mod.rs` returns nothing) and no svc call site passes `NoHooks`, or a non-`NoHooks` `impl CompactionHooks` exists outside `src/tests/`
- [ ] `cargo test -p cyrup-session` passes with no failures, and `cargo test -p cyrup-session-svc` still builds and passes
- [ ] `cargo clippy --all-targets -p cyrup-session -p cyrup-session-svc` reports 0 findings
- [ ] `cargo doc --no-deps -p cyrup-session` produces no new warnings relative to the current 4

## Verifying command

```bash
cd /home/user/cyrup && grep -rn 'apply_before_agent_start\|BeforeAgentStartHook' crates/ --include='*.rs' | grep -v crates/cyrup-session/ | wc -l; grep -rn 'CompactionError::Hook' crates/ --include='*.rs' | wc -l; grep -rn 'impl CompactionHooks for' crates/ --include='*.rs'; grep -rn 'Compactor::new' crates/ --include='*.rs' | grep -v '/tests/'
```
