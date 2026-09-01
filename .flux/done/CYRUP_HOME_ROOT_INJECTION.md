---
stage: exec
status: done
updated: 2026-08-31 14:03
---

# Apply R2 To The `CYRUP_HOME` Mutators

## Read first, in this order

1. [`docs/TEST-ARCHITECTURE.md` § R2](../../docs/TEST-ARCHITECTURE.md) (line 549) — the rule this
   task implements. Three tiers, in order of preference. **Do not invent a mechanism.**
2. [`paths.rs`](../../crates/cyrup-ext-subagents/src/paths.rs) module doc — states the design
   invariant this task must restore. It is currently **false**; see "The finding" below.
3. [`background/mod.rs:1290-1330`](../../crates/cyrup-ext-subagents/src/background/mod.rs) — the
   `temp_root_dir` doc, which independently counts the same 19 test files this task targets.

**R2's file paths predate the consolidation.** Every path it cites under `crates/*/tests/` has
moved — into `crates/cyrup-it/tests/subagents/` or into a crate's own `src/tests/`. Resolve its
citations by filename, not by path, and treat its counts as a 2026-08 snapshot rather than
current state.

## The finding that reshapes this task

The previous draft assumed **one** `CYRUP_HOME` ladder behind `paths.rs::home_dir()`, so a single
`home_root` config field threaded to `agent_dir()` callers would cover the 19 files. That is wrong.

`paths.rs:3-7` claims *"There is exactly one home ladder (`CYRUP_HOME` -> `HOME` -> `temp_dir`), one
`getAgentDir()` and one `getProjectConfigDir()` in the crate, so the same logical question can no
longer get different answers depending on which module asks it."*

**There are five, and they disagree.** Measured on the post-rebase tree:

| # | Resolver | Reads | Injectable core today |
|---|---|---|---|
| A | [`paths.rs:27`](../../crates/cyrup-ext-subagents/src/paths.rs) `home_dir()` | `CYRUP_HOME`→`HOME`→temp | partial — `resolve_agent_dir(home)` at `:42`, but `home_dir()` itself is not |
| B | [`background/mod.rs:1320`](../../crates/cyrup-ext-subagents/src/background/mod.rs) `temp_root_dir()` | `CYRUP_HOME` only | **yes** — `temp_root_dir_from(env, os_temp)` at `:1327` (private) |
| C | [`discovery/skills.rs:103`](../../crates/cyrup-ext-subagents/src/discovery/skills.rs) `SkillDiscoveryDirs::from_env()` | `CYRUP_HOME`→`HOME`→temp | struct fields are injectable; `from_env()` is not wired |
| D | [`native_supervisor.rs:1788`](../../crates/cyrup-ext-subagents/src/native_supervisor.rs) `intercom_agent_dir()` | `CYRUP_CODING_AGENT_DIR`, then `CYRUP_HOME`→`HOME` | **yes** — `intercom_agent_dir_from(env, cwd)` at `:1772` (`pub`) |
| E | [`spawn/nested_events.rs:83`](../../crates/cyrup-ext-subagents/src/spawn/nested_events.rs) `temp_root_dir()` | `CYRUP_SUBAGENTS_TEMP_ROOT` **only — never `CYRUP_HOME`** | no |

B and E are both documented as pi's `TEMP_ROOT_DIR`, resolve to **different directories**, and are
reached from different modules (`native_supervisor.rs:120` takes E; `run_artifact_roots` takes B).
A test that sets only `CYRUP_HOME` relocates B and not E — which is exactly why
`native_supervisor_channel_integration.rs` sets `CYRUP_SUBAGENTS_TEMP_ROOT` **as well**, and
`run_state_signal_and_stop_parity.rs` sets `TEMP_ROOT_ENV` as well.

Two more reads sit outside this crate and outside any fix made here:
[`cyrup-intercom/src/paths.rs:85`](../../crates/cyrup-intercom/src/paths.rs) and
[`cyrup-mcp/src/config.rs:1638`](../../crates/cyrup-mcp/src/config.rs).

**Doc rot found while measuring.** `dirs_home()` does not exist anywhere in `crates/*/src`. It is
referenced by `discovery/skills.rs:100` and by three test comments
(`companions_hostservices_proof.rs:98`, `foreground_progress_stream_integration.rs:111`,
`discovery_project_root_wiring_integration.rs:69`) as if it were the canonical resolver. Anyone
following those comments looks for a function that is gone.

## Why this is urgent now, and not when R2 was written

R2 warned the 33 per-file `static ENV_MUTATION_LOCK`s *"will silently stop working"* — future tense,
conditional on consolidation. **Consolidation has happened.** `crates/cyrup-ext-subagents/tests/`
no longer exists; every file R2 cites by that path now lives in
`crates/cyrup-it/tests/subagents/`, and `Cargo.toml:175-178` compiles all of them into **one**
`[[test]] name = "subagents"` binary.

So the prediction is now the state: **24 distinct mutexes in one binary guarding one shared
environment — no mutual exclusion at all.** Under `cargo test -p cyrup-it --features it` those 24
locks serialize nothing across files.

### What is actually holding it together

[`.config/nextest.toml:53-70`](../../.config/nextest.toml) already lands R2's pairing:

```toml
env-mutating = { max-threads = 1 }
filter = 'binary(subagents) or test(/env_/)'
```

nextest runs each test in its own process **and** this group serializes the whole `subagents` binary
to one test at a time. That is correct and it is why the suite is green. Two consequences the task
must state plainly:

- `cargo nextest run` is the **only** correct invocation today. Plain `cargo test -p cyrup-it
  --features it` is racy by construction, and no amount of per-file locking fixes it.
- The mitigation costs the entire seam suite its parallelism. **Narrowing that filter from
  `binary(subagents)` to the genuinely-irreducible remainder is the measurable payoff of this
  work** — not abstract rule compliance.

## Corrected measurements

Every number in the previous draft was stale. Measured on the current tree:

| Claim in prior draft | Actual | How to re-measure |
|---|---|---|
| 26 live `home_dir`/`agent_dir` sites, 9 modules | **29 raw hits / 17 modules**; 4 are comments, 3 are `cfg(test)` → **22 production** | `grep -rnE '\b(paths::)?(home_dir\|agent_dir)\(\)' src` |
| "~20 crate-owned" | **15**, across 10 modules | 22 production minus the 7 `~`-expansion sites |
| ~6 `~`-expansion sites | **7** — `chain_graph.rs:714,721,723`, `worktree.rs:306,498`, `executor/paths.rs:113`, `missions/store.rs:567` | |
| leave `registration/doctor.rs` | **vacuous** — `doctor.rs` never calls `home_dir()`/`agent_dir()` (0 `paths::` references). Drop this bullet. | `grep -c 'paths::' registration/doctor.rs` |
| 17 files mutate `CYRUP_HOME` | **19 files, 65 mutation sites** (61 literal + 4 via `CYRUP_HOME_ENV_VAR`) | `grep -rlE '(set_var\|remove_var)\(\s*"?CYRUP_HOME' tests/subagents` |
| 20 files declare a lock | **24** in the subagents binary; **27** repo-wide (R2 counted 33) | `grep -rl 'static ENV_MUTATION_LOCK\|static ENV_LOCK'` |
| "45 → 0 call sites" | R2's §10 table says *call sites*; R2's prose says **"45 files call it."** Neither matches today: `crates/cyrup-it/tests` alone holds **283 sites across 34 files**, and **87 files** repo-wide. Quote the table, but do not claim this slice moves a 45-site total. | |

The 19 files split by what else they mutate — this is what decides the order of work:

- **5 set `CYRUP_HOME` and nothing else** — `companions_hostservices_proof`, `cyrup_home_env_sandboxed_tests`,
  `fleet_inspector_integration`, `management_actions_tool_dispatch_integration`,
  `wait_tool_registration_integration`. Converting one of these takes a **whole file's lock out**,
  because no other mutation remains to guard.
- **12 also set the fixture binary/script vars** — these need Phase A's `spawn_command` injection
  *and* the home injection before their lock can go. Partial conversion leaves the lock in place
  (see conversion rules).
- **2 also set a temp-root var** (`native_supervisor_channel_integration`,
  `run_state_signal_and_stop_parity`) — these need resolver **E** as well as **B**.

## The prescribed solution

Threading `home_root: Option<PathBuf>` through call signatures does not work at this scale. The
temp-root family alone (`default_async_root` **15** production call sites, `default_results_dir`
**14**, `resolve_background_storage_roots` **2**, `resolve_wait_tool_enabled` **1**) would put a new
parameter on ~32 production sites whose callers do not hold a config — and that is one of two
families, before resolvers C, D and E.

**Make `paths.rs`'s stated invariant true, then inject once.** The crate already has the idiom —
`temp_root_dir_from`, `intercom_agent_dir_from`, `resolve_agent_dir` — it is simply not applied
uniformly. Unify first; injection then has exactly one place to attach.

### Phase 1 — collapse five ladders into one (no threading, no behaviour change)

Give `paths.rs` the injectable core its partners already have:

```rust
/// The home ladder with its one ambient input passed in, so every branch is provable without
/// moving process-global state. Mirrors `background::temp_root_dir_from` and
/// `native_supervisor::intercom_agent_dir_from`.
#[must_use]
pub fn home_dir_from(env: &dyn Fn(&str) -> Option<std::ffi::OsString>) -> PathBuf {
    env("CYRUP_HOME")
        .map(PathBuf::from)
        .or_else(|| env("HOME").map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir)
}

#[must_use]
pub fn home_dir() -> PathBuf {
    home_dir_from(&|k| std::env::var_os(k))
}
```

Then repoint the duplicates so exactly one ladder remains:

- **C** — `SkillDiscoveryDirs::from_env()` calls `crate::paths::home_dir()`; delete its private copy
  and the stale `extension.rs::dirs_home` reference at `skills.rs:100`.
- **B** — `temp_root_dir_from` keeps its `env` closure but resolves home through the shared ladder
  rather than re-reading `CYRUP_HOME` inline.
- **D** — `intercom_agent_dir_from` likewise, keeping its `CYRUP_CODING_AGENT_DIR` precedence.
- **E** — leave the resolution as-is (it reads a different variable and has upstream-parity reasons),
  but **correct its doc** so it no longer reads as the same `TEMP_ROOT_DIR` as B. Two roots with one
  name is the defect that makes tests set two variables.

This phase changes no signatures and no behaviour, and is independently verifiable: with no env set,
every resolver returns exactly what it returns today.

### Phase 2 — one context, attached where config already lives

`SubagentExtensionConfig` is already the injection vehicle — `spawn_command` proved the shape, and
[`extension/host/mod.rs:115`](../../crates/cyrup-ext-subagents/src/extension/host/mod.rs)
`with_config_and_cwd(config, cwd)` is the constructor the tests already call. Add beside
`default_session_dir`:

```rust
/// Overrides the root every cyrup path in THIS process resolves against — the `CYRUP_HOME`
/// ladder's answer, supplied directly.
///
/// # What this reaches, and what it does not
/// Reaches the unified ladder in `crate::paths` and therefore every resolver repointed at it in
/// Phase 1 (agent dir, skills roots, the background temp root, the intercom agent dir).
/// It does NOT reach `spawn::nested_events::temp_root_dir` (a different variable), any other
/// crate's resolver (`cyrup-intercom`, `cyrup-mcp`), or any DETACHED child — a detached runner is
/// a separate process and reads its own environment. For those, see tier 2.
#[serde(skip)]
pub home_root: Option<PathBuf>,
```

`#[serde(skip)]` for the reason `spawn_command` carries it: a `config.json` able to relocate where a
run writes artifacts is a hazard, and this override is for a caller holding the process.

Leave the 7 `~`-expansion sites alone — they expand a *user-supplied* path against the real home and
are not sandbox seams.

### Phase 3 — convert the 5 single-variable files first

They are the only files where one change removes a lock. Each is a direct `with_config_and_cwd`
caller already. Do these before touching the 12 mixed files.

### Phase 4 — tier 2 for the detached runners

R2: *"Where the env var IS the mechanism under test … set it on the child's `Command`, not on the
process."* `spawn_detached_runner_with_command` already takes `env_overlay: &BTreeMap<String, String>`
applied via `.envs()`. The worked example is
[`cyrup-tools/src/tests/bash_session_env.rs:235`](../../crates/cyrup-tools/src/tests/bash_session_env.rs)
— *"cyrup's bin declines the process-global `set_var`, so the tool pushes them per child"* —
with `:11` stating the file never mutates the process at all. (R2 cites this as
`crates/cyrup-tools/tests/bash_session_env.rs:204`; that path no longer exists.)
Do not conclude these must mutate the process — an earlier draft did, without checking R2.

### Phase 5 — tier 3 only if a remainder survives

Confirmed absent today: no `crates/cyrup-test-support/src/env.rs`, `lib.rs:23` is
`#![forbid(unsafe_code)]`, no root `clippy.toml`. `cyrup-test-support` is `publish = false`
(`Cargo.toml:10`), so the blast radius is the test layer. If built: relax `forbid`→`deny` (`forbid`
cannot be locally overridden) with exactly one documented `#[allow]`, and add R2's
`disallowed-methods` entries.

Note `cyrup_home_env_sandboxed_tests.rs` is **not** the automatic tier-3 candidate the prior draft
assumed. Its header says it needs the var so `init()`'s T6 housekeeping resolves async/results roots
under a tempdir — that is resolver **B**, reached through `run_artifact_roots`. Once B is injectable
it is a tier-1 conversion. Re-assess it after Phase 2, not before.

### Phase 6 — narrow the nextest filter

Replace `binary(subagents)` with a filter naming only the files that still mutate. This is the
payoff; without it the suite stays serialized regardless of how many files were converted.

## This task should be split

The task's own stop condition — *"If tier 1's threading balloons past the extension's own call
sites, stop and `/split`"* — has measurably triggered: five resolvers, ~32 production call sites in
the temp-root family alone, plus two out-of-crate readers. Phases 1 and 2 are each a session; phases
3–5 are per-file work gated on them.

Run `/split` on this file before `/exec`. Phase 1 is safely executable on its own if a single
session is wanted now — it is self-contained, changes no signatures, and makes every later phase
smaller.

## Conversion rules — each learned by breaking something

- **Remove a file's `set_var` and its `ENV_MUTATION_LOCK` in the same edit.** Removing the lock while
  any mutation remains turns a cross-file race into an intra-file one; a *shifting* failure set
  across runs is that signature, a fixed set is a real regression.
- **Partial conversion within a file is fine while the lock stays.** This is why the 12 mixed files
  cannot be finished by this task's home work alone.
- **Delete the lock's doc comment with the lock.** Three comments outlived their static in the last
  pass and floated onto unrelated functions, asserting serialization that no longer happened.
- **Run each file alone right after converting it.** Passing alone but not in the suite means state
  is still shared.
- **Never set a config field on a path that cannot consume it.** An injection into a detached spawn
  is inert and invites the next reader to delete the load-bearing `set_var`.
- **Read warnings, not just errors.** An orphaned `unused variable: script_path` was the only signal
  that a scripted conversion had missed a helper.
- **Verify with `-p cyrup-it --all-targets --features it`.** `cargo check --workspace --all-targets`
  passes clean while never building these targets at all — `required-features = ["it"]` gates them
  out. That gap has already hidden one compile break on this branch.

## Definition of done

- [ ] Exactly one `CYRUP_HOME` ladder remains in `cyrup-ext-subagents`; `paths.rs`'s module doc is
      true as written; resolvers B, C and D resolve through it.
- [ ] `skills.rs:100`'s `extension.rs::dirs_home` reference is gone, and E's doc no longer names
      itself pi's `TEMP_ROOT_DIR` in the same terms as B.
- [ ] `home_root` exists with `#[serde(skip)]` and a field doc naming what it reaches **and** what it
      does not (detached children, resolver E, other crates).
- [ ] The 7 `~`-expansion sites are untouched; with no `home_root` configured, every resolver returns
      exactly what it returns today.
- [ ] The 5 single-variable files are converted and their locks removed.
- [ ] Detached-runner files use `env_overlay`/`Command::env`, not process mutation.
- [ ] Tier 3 built **only if** a genuine remainder survives; if built, `forbid`→`deny` with exactly
      one `#[allow]`.
- [ ] The nextest `env-mutating` filter names the remaining files instead of `binary(subagents)`.
- [ ] Report states the measured before/after for: files mutating `CYRUP_HOME` (from 19), mutation
      sites (from 65), and locks in the subagents binary (from 24).
- [ ] `cargo nextest run -p cyrup-it --features it` green; `cargo test -p cyrup-ext-subagents` green;
      clippy clean on `-p cyrup-it --all-targets --features it`.


---

## Outcome

Delivered. Two DoD items were met differently than written; both deviations are
deliberate and were verified, not assumed.

**Item 1 — "resolvers B, C and D resolve through [one ladder]" was WRONG and is not
done as written.** C was a byte-identical copy and was deduplicated. B and D
deliberately differ: B (`background::temp_root_dir`) has no `HOME` rung, so routing
it through `paths::home_dir` would put reboot-disposable run scratch in the user's
real `$HOME` whenever `CYRUP_HOME` is unset — its state everywhere except sandboxed
tests. D carries an extra `std::env::home_dir` rung and is pinned byte-identical to
`cyrup_intercom::paths::agent_dir_path_from` across a dependency edge that forbids
importing it. Unifying either is a behaviour change, not a cleanup. `paths.rs`'s
module doc now states the truth — four ladders, why each differs — instead of the
false "exactly one" claim that motivated this item.

**Item 8 — the nextest filter was DETACHED, not narrowed.** No filter selects the
`env-mutating` group now, because nothing in the workspace needs it: process-env
mutation races only within a binary (nextest gives each test its own process), and
no multi-test binary mutates any more. The group is kept unattached so the next
legitimate mutator is pinned rather than re-deriving why. Measured: the subagents
suite went 61.8s -> 15.9s.

**Tier 3 was never built and is not needed.** Its premise did not survive contact:
every candidate, including the ones whose variables genuinely ARE the mechanism
under test, reduced to tier 1 or 2 once the crate's existing injectable seams were
used rather than assumed absent. Scrubbing an ambient variable — the case that
looked irreducible — is just `None` from an injected lookup. Both
`#![forbid(unsafe_code)]` declarations stand unchanged; no exception was spent.

### Measured

| | Before | After |
|---|---:|---:|
| Files mutating `CYRUP_HOME` (tests/subagents) | 19 | **0** |
| `CYRUP_HOME` mutation sites | 65 | **0** |
| Locks in the subagents binary | 24 | **0** |
| All env mutations in tests/subagents | 250 | **0** |
| Per-file env mutexes, repo-wide | 33 | **1** |
| Disallowed-method violations, workspace | — | **0** (5 documented exemptions) |
| Subagents suite runtime | 61.8s | **15.9s** |

### Beyond the original scope

- `paths::Roots` — the ambient roots resolved once at a boundary and carried, rather
  than re-derived at ~30 call sites. This is what closed the cascade case I had
  wrongly written off as needing "a new parameter through four control-flow
  functions"; those four already shared a `TurnLoopIo`.
- `clippy.toml` `disallowed-methods` — R2's reviewer check, proven to fire. It found
  `tests/intercom/child_bridge_activation.rs`, a file outside this task's scope that
  would otherwise have shipped with four mutations.
- `RunOptions::child_env` and `RunnerOverrides::{roots, child_env}` — the tier-2
  foreground twin of the detached `env_overlay`, and the in-process runner seam.
- Two "flaky" tests fixed as real defects: `late_interrupt` (a 20ms margin that
  inverts under load, now anchored on the child's own progress) and
  `a_quickly_exiting_detached_child` (asserting a fixture built to exit instantly is
  still alive).

### Verified

486/486 across all seven `cyrup-it` binaries under load; 2587 `cyrup-ext-subagents`
tests unchanged throughout; clippy clean; 0 violations under
`--workspace --all-targets --all-features`.
