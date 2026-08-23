---
stage: new
status: done
updated: 2026-08-23 00:47
---

# Narrow cyrup-session's Public API Surface

> Found by a six-lens hygiene audit of `crates/cyrup-session`, run after the `manager/`
> decomposition landed in PR #53. Every claim below was reproduced against the tree.
> **Priority:** medium · **Effort:** medium

cyrup-session publishes machinery no downstream crate can reach, and simultaneously omits from its curated facade the one function downstream actually needs. Both directions were measured across `crates/` and `xtask/` excluding cyrup-session itself.

## Description

cyrup-session publishes internal-only machinery as crate API: four modules that no other crate imports, fifteen root re-exports with zero external consumers, a facade gap that forces callers down a deep path, and an unused `futures` dependency edge. Narrowing the surface makes the crate's real contract legible.

**This task is gated** — see the block above. In a conformance-tracked port an unreferenced `pub` item is usually unfinished wiring rather than dead API, so each identifier must clear `docs/gap-analysis/` individually before it is touched.

## 1. Internal-only public modules

- **`pub mod store`** (`lib.rs:30`) + `pub use store::{DiskStore, MemStore, SessionStore}` (`lib.rs:57`): `grep -rn 'pub \(async \)\?fn .*\(SessionStore\|DiskStore\|MemStore\)' crates/cyrup-session/src/` returns **nothing** — the persistence seam appears in no public signature. `SessionManager.store` is a private field and the only constructors taking `Box<dyn SessionStore>` (`assemble`, `adopt_branch`) are private. Internal users are `manager/mod.rs:40,45,63`, `manager/branched_session.rs`, `manager/lifecycle.rs` only.
- **`pub mod ids`** (4 pub fns) and **`pub mod migrate`** (1 pub fn): the only 2 of the 14 pub modules lib.rs does *not* re-export at root. `grep -rho 'cyrup_session::ids\b' ...` and same for `migrate`/`store` → **0, 0, 0**.
- **`pub mod prompt::skills_inject`** (`prompt/mod.rs:26`): `grep -c '^pub ' src/prompt/skills_inject.rs` → **0**. It is an empty public module — its single fn is `pub(crate)` (`skills_inject.rs:33`). This is also the root cause of the rustdoc warning at `prompt/builder.rs:261`.

Demote all four to `pub(crate) mod` / `mod` and drop the three store names from the root re-export.

## 2. Fifteen root re-exports with zero external consumers

`BeforeAgentStartHook`, `BeforeAgentStartInput`, `BeforeAgentStartOutput`, `apply_before_agent_start`, `TrustQuery`, `CURRENT_VERSION`, `ContextDiagnostic`, `DEFAULT_SELECTED_TOOLS`, `DiskStore`, `MemStore`, `SessionStore`, `SessionListProgress`, `canonicalize_path`, `list_all_with_progress`, `newest_session` — each `grep -rw <name> crates/ xtask/ --include='*.rs' | grep -v '^crates/cyrup-session/' | wc -l` → **0**. Internal-only users: `canonicalize_path` from `prompt/context_files.rs:151,184,185,200`; `newest_session` from `manager/lifecycle.rs:149`; `DEFAULT_SELECTED_TOOLS` from `prompt/builder.rs:332`.

Narrowing these lets the compiler's `dead_code` lint start doing this audit automatically — today `pub` suppresses it, which is why the dead items in DEAD_PUB_ITEMS sat undetected despite clippy reporting 0 findings. (The 4 `BeforeAgentStart*` names are owned by DEAD_HOOK_SEAMS — skip them here if that task lands first.)

## 3. Facade gap forcing a deep path

`compaction/mod.rs:52-55` curates a flat facade and all 8 `use cyrup_session::compaction::{...}` sites in cyrup-session-svc import flat names — but three `pub fn`s in `tokens.rs` are missing from the list: `estimate_agent_message` (`tokens.rs:58`), `estimate_custom_message_content` (`:73`), `estimate_summary_text` (`:80`). Result: `estimate_agent_message` is the only item in the whole crate reached via a deep submodule path, at 3 sites:

```
cyrup-session-svc/src/session/auto_compaction.rs:296
cyrup-session-svc/src/session/compaction.rs:204
cyrup-session-svc/src/tests/compaction_tokens_after.rs:35
```

This is the *only* reason `pub mod tokens` must stay public — the other 9 compaction submodules have 0 external references each. Add the three names to the `pub use tokens::{…}` list, update the 3 call sites, then the 10 `pub mod` declarations at `compaction/mod.rs:10-19` can all become `pub(crate) mod`.

## 4. Small cleanups in the same pass

- **Unused dependency**: `Cargo.toml:23` declares `futures.workspace = true` and `cargo tree -p cyrup-session -e normal --depth 1` shows `futures v0.3.32` as a direct edge, but `grep -rn futures crates/cyrup-session/src --include='*.rs' | wc -l` → **0**. Delete the line.
- **Duplicate re-export**: `SkillPointer` (owned by cyrup-resources) is re-exported by cyrup-session twice — `prompt/mod.rs:41` and `lib.rs:54` — giving it 3 reachable paths. Downstream ignores both and takes it from cyrup-resources directly. Drop the `lib.rs` one.
- **Bare verbs at crate root**: `lib.rs:46-49` hoists `list` and `resolve`, which sit beside the unrelated `resolve_path` from `git_paths` (`lib.rs:43`). Neither root name has any caller (`grep -rn 'cyrup_session::\(list\b\|resolve\b\)' ...` → 0); real imports go through `cyrup_session::listing::{...}`. Leave them reachable only as `listing::list` / `listing::resolve`.

## Acceptance Criteria

- [ ] `store`, `ids`, `migrate` and `prompt::skills_inject` are no longer `pub mod`; `grep -n 'pub mod \(store\|ids\|migrate\)' crates/cyrup-session/src/lib.rs` and `grep -n 'pub mod skills_inject' crates/cyrup-session/src/prompt/mod.rs` return nothing
- [ ] `compaction/mod.rs` re-exports `estimate_agent_message`, `estimate_custom_message_content` and `estimate_summary_text` from the flat facade, and `grep -rn 'compaction::tokens::' crates --include='*.rs' | grep -v 'crates/cyrup-session/'` returns 0 hits
- [ ] No name remaining in lib.rs's root `pub use` block has zero external references, except ones deliberately documented as future API in a comment
- [ ] `grep -n futures crates/cyrup-session/Cargo.toml` returns nothing and `cargo tree -p cyrup-session -e normal --depth 1` no longer lists futures
- [ ] `SkillPointer` appears in exactly one re-export inside cyrup-session, and root-level `list`/`resolve` are gone from `lib.rs`
- [ ] `cargo build --workspace` and `cargo test -p cyrup-session -p cyrup-session-svc` pass; `cargo clippy --all-targets --workspace` reports 0 findings

## Evidence

```bash
cd /home/user/cyrup && for m in store ids migrate; do printf '%-10s %s\n' "$m" "$(grep -rho "cyrup_session::$m\b" --include='*.rs' crates xtask | wc -l)"; done; grep -c '^pub ' crates/cyrup-session/src/prompt/skills_inject.rs; grep -rn 'compaction::tokens::' crates --include='*.rs' | grep -v 'crates/cyrup-session/'; grep -rn futures crates/cyrup-session/src --include='*.rs' | wc -l
```
