---
title: Public Api Changes Beyond The Async Keyword
priority: LOW
stage: qa
status: completed
updated: 2026-08-23 08:20
---

# The 28-item public-surface record — and the one rustdoc hunk that is still an open change

## Verdict

The reviewer's enumeration is **correct in its counts and in its four callouts**. It is an
inventory, not a defect, so the first question is what the deliverable is. Three candidates were
considered:

| Candidate | Decision | Why |
| --- | --- | --- |
| (a) A CHANGELOG / release-note entry somewhere in the repo | **Rejected** | The workspace has no changelog, no `.github/`, no release pipeline, and every crate is `version = "0.0.0"`. `README.md:9` states *"not a released product."* There is no convention to add to, and a LOW inventory task is the wrong place to invent one — see §5. |
| (b) A deprecation shim or softening re-export for some of the four | **Rejected** | Every consumer of all four is in-workspace and named in §2. A shim would have to pick a `CancelToken` on the caller's behalf (`FileLock::acquire`), or preserve a nominal type the other two tasks have already ruled correct as-is. See §7. |
| (c) Purely informational — close as recorded | **Adopted, with one exception** | The record IS the deliverable. **The exception**: [`MEDIUM-with-lock-let-next-comments-invent-a-borrowck-rule.md`](MEDIUM-with-lock-let-next-comments-invent-a-borrowck-rule.md) explicitly hands this file two things it declined to do — the *"why did `read` stay sync"* sentence, and the decision on whether `read` should become async at all. Those are answered and specified in §6 and §7. |

**Required path: the single rustdoc hunk in §6.** One file, one doc-comment block, zero non-comment
lines. Nothing else in this task changes any source file.

> ### Citation policy for this file
>
> Every `file:line` below was re-verified against the working tree on 2026-08-23, **after** the
> `ops/local.rs` decomposition, the `crates/cyrup/src/main.rs` split into `bootstrap.rs` /
> `prelaunch.rs` / `interactive.rs` / `actions.rs` / `session_launch.rs` / `predispatch.rs`, and the
> two earlier passes over `cyrup-config/src/lock.rs` and `cyrup-config/src/models_store.rs`.
> Corrections made in this pass are listed in §0. Where a pointer can be expressed by **name**, it
> is — a nameless reference cannot rot, and this queue has already shipped three rounds of stale
> line numbers.

---

## 0. Citations corrected in this pass (drift log)

| Was | Is now | Item |
| --- | --- | --- |
| `cyrup-config/src/lock.rs:57` | `lock.rs:103` | `pub async fn acquire` |
| `cyrup-config/src/lock.rs:29` | `lock.rs:34` | `static NEVER_CANCELLED` |
| `cyrup-config/src/models_store.rs:234` | `models_store.rs:252` | `FileModelsStore::read_latest` |
| `main.rs:176 / :343 / :551 / :601 / :1568` | `bootstrap.rs:62`, `:98`, `:215`, `:267`; `prelaunch.rs:268` | the five `SettingsManager::load` call sites that used to live in `main.rs` (still 8 non-test call sites in total, see §7.1) |
| "six call sites" of `FileLock::acquire` | **eight** non-test call sites | §2 (b) |
| `cyrup-tools/src/ops/mod.rs:17-21` | `ops/mod.rs:17-20` (`:21` is the unrelated `shell` re-export) | §1 negative result |
| "421 occurrences of `[CYRUP-DELTA]`" | **420** occurrences of `CYRUP-DELTA` in `.rs` files under `crates/`, **303** of them in the bracketed `[CYRUP-DELTA]` form | §5 |
| "`docs/` holds `adr/`, `gap-analysis/`, `audits/`, `guide/` (the mdBook, `book.toml`), `PARITY-PLAN.md`, `TEST-ARCHITECTURE.md`" | same, plus `models.md` and `providers.md`; **`book.toml` is at the repo ROOT**, not under `docs/` | §5 |
| "Both `cyrup-core` and `cyrup-config` take a new `dashmap` dependency" | `cyrup-core`, `cyrup-config` **and `cyrup-tools`** all declare `dashmap = { workspace = true }` (`Cargo.toml:18`, `:22`, `:25` respectively) | §4 |

Verified-and-unchanged (spot-checked because they are load-bearing): `store.rs:9-12` (trait
rustdoc), `store.rs:13` (`#[async_trait::async_trait]`), `store.rs:20-24`, `store.rs:51-61`,
`store.rs:118-120`, `manager.rs:26`, `manager.rs:47`, `:77-78`, `:94`, `:139`, `:174`, all ten
`SettingsManager` setters in §3, all three `TrustStore` methods, `builder.rs:377`, `:438-445`,
`:689`, `factory.rs:27`, `startup.rs:235`, `:258`, `:299`, `startup_ui.rs:401`,
`accessors.rs:103/115/130`, `management.rs:1337`, `:1421`, `settings_write.rs:152/183/213`,
`cyrup-tools/src/lock.rs:85`, `:140-146`, `cyrup-core/src/lib.rs:16`, `:33`,
`cyrup-sdk/src/lib.rs:76-81`, `:110`, `:114`, `Cargo.toml:87`, `cyrup-it/Cargo.toml:22`,
`README.md:9`, `README.md:26-28`, `docs/adr/ADR-0008-…` §C (`:295`) and its `sdk.ts:114-126`
clause (`:318-319`).

---

## 1. The count

**28 confirmed**, but the buckets are not quite the ones the review drew. The review's bucket (a)
holds 24 "`async` keyword only" items; it is actually **23**, because the 24th —
`FileMutationLocks::guard` — did not gain `async` (it already had it) and did not stay unchanged
either: its **return type changed identity** when `MutationGuard` became an alias. It belongs with
the four, as a consequence of one of them. Corrected split: **23 mechanical + 5 that a downstream
consumer notices for a reason other than adding `.await`.**

| Bucket | Count | Items |
| --- | --- | --- |
| (a) `async` keyword only, nothing else | 23 | §3 |
| (b) parameter added | 1 | `FileLock::acquire` |
| (c) type changed | 3 | `SettingsStore::with_lock`, `TrustPromptFn`, `MutationGuard` |
| (d) declaration text unchanged, return type identity changed | 1 | `FileMutationLocks::guard` |
| **total** | **28** | |

Everything else that touches a `pub` line is either a call site gaining `.await`, or rustfmt reflow
(`ops/shell.rs`, `config.rs`) that
[`LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md`](LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md)
owns.

**A negative result worth recording, because it is the largest single restructuring on the branch.**
The `ops/local.rs` → `ops/local/` decomposition moves **zero** public API. Every submodule is
`pub(crate)` ([`ops/local/mod.rs:25-30`](../../crates/cyrup-tools/src/ops/local/mod.rs) —
`command`, `fs`, `guard`, `proc`, `signal`, `tracking`) and every previously public item is
re-exported at its original path: `LocalFs` (`local/mod.rs:37`), `LocalProc` (`:38`), `kill_pid`,
`kill_process_tree`, `terminate_pid` (`:39`), and `kill_tracked_detached_children`,
`track_detached_child_pid`, `untrack_detached_child_pid` (`:40`), with
[`ops/mod.rs:17-20`](../../crates/cyrup-tools/src/ops/mod.rs) re-exporting the six free functions
onward. `cyrup_tools::ops::local::LocalFs` still resolves. A reader skimming by line count would
guess the opposite.

---

## 2. The five non-mechanical changes

### (b) Parameter added — breaking

| Item | Change |
| --- | --- |
| [`cyrup_config::lock::FileLock::acquire`](../../crates/cyrup-config/src/lock.rs) (`lock.rs:103`) | was `pub fn acquire(target: &Path) -> Result<Self, ConfigError>`; is `pub async fn acquire(target: &Path, cancel: Option<&CancelToken>) -> Result<Self, ConfigError>` |

Every out-of-tree caller must pass a second argument. In-tree there are **eight non-test call
sites**, five passing `None` and three (all in `models_store`) forwarding a real signal:

| Call site | Second argument |
| --- | --- |
| `cyrup-config/src/settings/store.rs:69` (`FileSettingsStore::with_lock`) | `None` |
| `cyrup-config/src/trust.rs:150` (`TrustStore::nearest`) | `None` |
| `cyrup-config/src/trust.rs:174` (`TrustStore::set_many`) | `None` |
| `cyrup-config/src/auth.rs:316` | `None` |
| `cyrup-ext-subagents/src/discovery/settings_write.rs:84` (`lock_settings_file`) | `None` |
| `cyrup-config/src/models_store.rs:267`, `:330`, `:357` | `options.and_then(|o| o.signal.as_ref())` |

The semantics of that parameter — and the fact that it does not reach the `flock` wait — belong to
[`MEDIUM-cancel-token-does-not-reach-the-cross-process-flock-wait.md`](MEDIUM-cancel-token-does-not-reach-the-cross-process-flock-wait.md)
and [`HIGH-dropped-acquire-future-detaches-blocking-flock-task.md`](HIGH-dropped-acquire-future-detaches-blocking-flock-task.md).

### (c) Type changed — breaking

| Item | Change | Owner |
| --- | --- | --- |
| [`cyrup_config::settings::SettingsStore::with_lock`](../../crates/cyrup-config/src/settings/store.rs) (`store.rs:20-24`) | was `fn with_lock(&self, scope, f: &mut dyn FnMut(Option<&str>) -> Option<String>)`; is an `async fn` on an `#[async_trait]` trait with `f: &mut (dyn for<'s> FnMut(Option<&'s str>) -> Option<String> + Send)` | §6 here, plus [`MEDIUM-with-lock-…`](MEDIUM-with-lock-let-next-comments-invent-a-borrowck-rule.md) |
| [`cyrup_session_svc::TrustPromptFn`](../../crates/cyrup-session-svc/src/builder.rs) (`builder.rs:438-445`) | returned `Option<bool>`; now returns `Pin<Box<dyn Future<Output = Option<bool>> + Send + 'a>>` under a `for<'a>` binder | [`MEDIUM-trustpromptfn-public-break-and-its-misstated-cost.md`](MEDIUM-trustpromptfn-public-break-and-its-misstated-cost.md) |
| [`cyrup_tools::lock::MutationGuard`](../../crates/cyrup-tools/src/lock.rs) (`lock.rs:85`) | was `pub struct MutationGuard { … }`; is `pub type MutationGuard = KeyedGuard<PathBuf>` | [`LOW-mutationguard-alias-erases-the-lock-domain-type-distinction.md`](LOW-mutationguard-alias-erases-the-lock-domain-type-distinction.md) |

`SettingsStore` is the one that changes three things at once, and an external implementor gets all
three compiler errors together: the trait now needs the `async_trait` proc macro to implement, the
closure parameter gained `Send`, and it gained an explicit higher-ranked lifetime. It is consumed
as `Arc<dyn SettingsStore>` in four crates —
[`cyrup-config/src/settings/manager.rs:26`](../../crates/cyrup-config/src/settings/manager.rs)
(the `SettingsManager::store` field),
[`cyrup-session-svc/src/builder.rs:377`](../../crates/cyrup-session-svc/src/builder.rs) and
[`factory.rs:27`](../../crates/cyrup-session-svc/src/factory.rs), and
[`cyrup/src/startup.rs:299`](../../crates/cyrup/src/startup.rs) (`file_settings_store`'s return
type, the workspace's only constructor of the file-backed impl).

### (d) Same declaration, different return type

| Item | What actually changed |
| --- | --- |
| [`cyrup_tools::lock::FileMutationLocks::guard`](../../crates/cyrup-tools/src/lock.rs) (`lock.rs:140-146`) | Signature text is unchanged: `pub async fn guard(&self, path: &Path, cancel: &CancelToken) -> Result<MutationGuard, ToolError>`. But `MutationGuard` is now `KeyedGuard<PathBuf>`, so the returned type is a different, foreign, generic type. The body is now a single delegation, `self.inner.guard(key, cancel).await.map_err(|_| error::aborted())` (`lock.rs:146`). |

This is the item most likely to be mis-triaged by a future reader: a signature-level audit reports
"unchanged", and only following the alias reveals it. Recorded here for exactly that reason; the
consequences are [`LOW-mutationguard-…`](LOW-mutationguard-alias-erases-the-lock-domain-type-distinction.md)'s.

---

## 3. The 23 mechanical ones — `async` keyword, nothing else

Each is a one-token `fn` → `async fn` change, with the parameter list, return type and generics
otherwise unchanged.

| Crate | Items | Count |
| --- | --- | --- |
| `cyrup-config` — [`SettingsManager`](../../crates/cyrup-config/src/settings/manager.rs) | `set` (`:219`), `set_nested` (`:277`), `persist_nested` (`:323`), `set_mermaid_rendering_mode` (`:360`), `set_editor_padding_x` (`:372`), `set_autocomplete_max_visible` (`:378`), `set_image_width_cells` (`:384`), `set_http_idle_timeout_ms` (`:394`), `set_show_images` (`:409`), `set_enable_analytics` (`:422`) | 10 |
| `cyrup-config` — [`TrustStore`](../../crates/cyrup-config/src/trust.rs) | `nearest` (`:149`), `set_many` (`:170`), `set` (`:198`) | 3 |
| `cyrup-ext-subagents` | [`handle_management_action`](../../crates/cyrup-ext-subagents/src/discovery/management.rs) (`management.rs:1421`), [`merge_builtin_agent_override`](../../crates/cyrup-ext-subagents/src/discovery/settings_write.rs) (`settings_write.rs:152`), `remove_builtin_agent_override` (`:183`), `remove_builtin_agent_override_fields` (`:213`) | 4 |
| `cyrup-session-svc` — [`AgentSession`](../../crates/cyrup-session-svc/src/session/accessors.rs) | `saved_trust_decision` (`:103`), `write_project_trust` (`:115`), `persist_setting` (`:130`) | 3 |
| `cyrup` (lib target) | [`apply_first_time_setup`](../../crates/cyrup/src/startup.rs) (`startup.rs:235`), `run_first_time_setup` (`:258`), [`run_trust_prompt`](../../crates/cyrup/src/startup_ui.rs) (`startup_ui.rs:401`) | 3 |
| | **total** | **23** |

**One near-miss, recorded so a future reader does not re-flag it.**
`handle_management_action`'s third parameter is written `&ManagementRequest<'_>` where it read
`&ManagementRequest`. That is **not** a signature change: `pub struct ManagementRequest<'a>` already
carried the lifetime (`management.rs:1337`), and the `<'_>` is explicit elision the `async`
conversion made the compiler ask for.

**Private signature changes**, listed only so the (b) audit is provably complete — none is `pub`:

- [`FileModelsStore::read_latest`](../../crates/cyrup-config/src/models_store.rs)
  (`models_store.rs:252`) gained `options: Option<&ModelsStoreOperationOptions>` and became
  `async`. It is a private inherent method (in `impl FileModelsStore`, `:216`) on a `pub struct`
  (`:139`); the rationale in its doc is
  [`LOW-models-store-rationale-still-rests-on-a-sync-read-latest.md`](LOW-models-store-rationale-still-rests-on-a-sync-read-latest.md)'s.
- `lock_settings_file` (`settings_write.rs:81`), `saved_trusted`
  ([`cyrup/src/subcommands.rs:438`](../../crates/cyrup/src/subcommands.rs)) and
  `persist_trust_choice` ([`cyrup/src/startup_ui.rs:386`](../../crates/cyrup/src/startup_ui.rs))
  became `async`.
- `handle_disable` (`management.rs:3459`), `handle_enable` (`:3524`) and `handle_reset` (`:3607`)
  became `async` and picked up the same `<'_>` elision as their public caller.

---

## 4. Additive surface — new, and it does reach the embedder crate

[`cyrup-core/src/lib.rs`](../../crates/cyrup-core/src/lib.rs) gains `pub mod keyed_lock;` (`:16`)
and root re-exports of `Cancelled`, `KeyedGuard`, `KeyedLockMap`, `KeyedLocks` (`:33`).
`dashmap = { workspace = true }` is declared by `cyrup-core` (`Cargo.toml:18`), `cyrup-config`
(`:22`) and `cyrup-tools` (`:25`).

This is the **only** part of the change set that lands on the ADR-0008 embedder surface, because
[`cyrup-sdk/src/lib.rs:110`](../../crates/cyrup-sdk/src/lib.rs) re-exports `pub use cyrup_core as
core;` — so `cyrup_sdk::core::keyed_lock::{KeyedLocks, KeyedLockMap, KeyedGuard, Cancelled}`
resolves today, along with `dashmap`'s full mutating surface through the `KeyedLockMap` alias. That
consequence is [`LOW-keyed-lock-map-alias-exposes-the-raw-dashmap.md`](LOW-keyed-lock-map-alias-exposes-the-raw-dashmap.md)'s;
recorded here because no other task states that the alias is reachable from `cyrup-sdk`.

**Embedder reachability of the rest, checked crate by crate** (this is what decides how far a break
travels, and no other task computes it):

| Item | Reachable from `cyrup-sdk`? | Path |
| --- | --- | --- |
| `keyed_lock::*` | **yes**, new | `cyrup_sdk::core::keyed_lock::…` (`lib.rs:110`) |
| `AgentSession::{saved_trust_decision, write_project_trust, persist_setting}` | **yes** | `AgentSession` is a named re-export (`lib.rs:76-81`) |
| `TrustPromptFn` | **yes**, indirectly | `cyrup_sdk::session_svc::TrustPromptFn` via `pub use cyrup_session_svc as session_svc;` (`lib.rs:114`) |
| `SettingsStore`, `SettingsManager`, `TrustStore`, `FileLock` | **no** | `cyrup-sdk/Cargo.toml` has no `cyrup-config` dependency (its only foundational one is `cyrup-core`, `:15`), and ADR-0008 §C rules that re-export out on purpose |
| `MutationGuard`, `FileMutationLocks` | **not yet** | `cyrup-sdk` has no `cyrup-tools` dependency **today**, but [ADR-0008 §C](../../docs/adr/ADR-0008-requirement-ids-and-sdk-surface.md) (`:295`) files a task to add one and re-export *"the file lock/mutation-queue primitive"* (`ADR-0008:318-319`, citing `sdk.ts:114-126`). The alias change lands on a type already scheduled to become embedder-visible. |

---

## 5. What this repo does with a breaking change — and why the answer is "nothing"

Searched, not assumed:

- **No `CHANGELOG`** anywhere: `find . -iname 'CHANGELOG*'` (excluding `target/`, `tmp/`) returns
  nothing.
- **No `.github/`** directory, so no release workflow.
- **No versions**: `[workspace.package] version = "0.0.0"`
  ([`Cargo.toml:87`](../../Cargo.toml)); no crate overrides it. Only `cyrup-it`
  (`cyrup-it/Cargo.toml:25`) and `cyrup-test-support` (`:10`) set `publish = false`, and
  `cyrup-it/Cargo.toml:22` says of that flag *"it is a registry flag and has no effect
  whatsoever"* — i.e. the repo already treats registry semantics as inapplicable.
- **`README.md:9`**: *"**Status:** not a released product."*
- **No `docs/` migration file.** `docs/` holds `adr/` (11 ADRs plus `README.md` and
  `LEADS-SETTLED.md`), `gap-analysis/`, `audits/`, `guide/` (the mdBook sources; `book.toml` is at
  the repo root), `PARITY-PLAN.md`, `TEST-ARCHITECTURE.md`, `models.md`, `providers.md`. None is a
  change log; `gap-analysis/` tracks divergence from **pi**, not from cyrup's own past.
- **The one convention that does exist for "a reader will be surprised by this"** is an in-source
  `CYRUP-DELTA` note naming the upstream line and the reason — 420 occurrences in `.rs` files under
  `crates/`, 303 of them in the bracketed `[CYRUP-DELTA]` form, the policy stated at
  `README.md:26-28`. In rustdoc the established spelling is a bolded marker opening the paragraph,
  `/// **[CYRUP-DELTA]** …` (`model/validate.rs:246`, `model/schema.rs:13`,
  `provider_compose.rs:169`). That convention is about pi-vs-Rust mechanism differences, which is a
  different axis from API stability — but it is exactly why §6's deliverable is a **rustdoc
  comment** and not a file: in this repo, the place a non-obvious mechanism choice gets recorded is
  next to the declaration that made it.

**The one public-surface policy that exists is [ADR-0008](../../docs/adr/ADR-0008-requirement-ids-and-sdk-surface.md)
§C** (`:295`), and all 28 items still satisfy it. Its rule is a *parity* rule — every capability pi
exports must exist, be `pub`, and be reachable from a published workspace crate — not a stability
rule. Nothing here was removed, un-`pub`'d, or made unreachable, so nothing trips it. Worth stating
plainly, because "28 public signatures moved" reads like an ADR-0008 event and is not one.

**Conclusion: creating `CHANGELOG.md` would be inventing a repo-wide convention from inside a LOW
inventory task, for a workspace with zero releases and zero out-of-tree consumers. Do not.** If a
maintainer later wants one, it is a repo-level decision belonging in `docs/adr/`. This file is the
record for this change set, and it is checked in (`.flux/README.md`: *"the queue travels with the
repo, so it is visible in review, survives a fresh clone"*), which is the property a changelog
entry would have bought.

---

## 6. The one required change — exact, verbatim

One file, one hunk, twelve added lines, all of them `///`:
[`crates/cyrup-config/src/settings/store.rs`](../../crates/cyrup-config/src/settings/store.rs).

[`MEDIUM-with-lock-…`](MEDIUM-with-lock-let-next-comments-invent-a-borrowck-rule.md) lists the
trait-level paragraph under **Do not touch** and assigns it here verbatim: *"The 'why did `read`
stay sync' question belongs to `LOW-public-api-changes-beyond-the-async-keyword.md`."* That task
owns the `with_lock` **method** rustdoc (its Replacement 1) and both impl bodies; this task owns the
**trait** paragraph above them. The two regions are disjoint.

The gap: the existing paragraph explains why `#[async_trait]` was chosen over a native `async fn`,
but not why the trait ends up **half** async — `read` sync, `with_lock` not. A contributor reading
only the declaration sees an inconsistency with no stated reason, and the obvious "fix" is to box
`read` too.

### 6.1 The edit

Anchor by content, never by line number. The anchor string below occurs **exactly once** in
`store.rs` — assert that before editing:

```
grep -c 'and a native async fn is not dyn-compatible\.$' crates/cyrup-config/src/settings/store.rs
```

must print `1`.

**Find this exact text** (two consecutive lines; currently `store.rs:12-13`):

```rust
/// `Arc<dyn SettingsStore>` (`manager.rs:26`), and a native async fn is not dyn-compatible.
#[async_trait::async_trait]
```

**Replace it with this exact text:**

```rust
/// `Arc<dyn SettingsStore>` (`manager.rs:26`), and a native async fn is not dyn-compatible.
///
/// **[CYRUP-DELTA]** The trait is HALF async on purpose. `with_lock` is `async` only because
/// [`crate::lock::FileLock::acquire`] is — that lock awaits an in-process keyed mutex and then a
/// non-blocking `flock` retry loop. `read` takes no lock in either impl: `FileSettingsStore::read`
/// is a bare `std::fs::read_to_string`, and [`InMemorySettingsStore`]'s is a `std::sync::Mutex`
/// lock plus a clone. It therefore has no suspension point at all, and `async` there would box a
/// future that never yields. It would also cascade: `read`'s only caller is
/// `SettingsManager::load_scope`, under `reload_internal`, under the sync `SettingsManager::load`,
/// `reload` and `set_project_trusted`. Upstream is sync on BOTH halves — `lockfile.lockSync`
/// behind a busy-wait retry loop annotated "Sleep synchronously to avoid changing callers to
/// async" (settings-manager.ts:206/218 @v0.83.0) and `readFileSync` (`:237`) — so the `async` is
/// confined to the half that takes a lock. Do not "finish the job".
#[async_trait::async_trait]
```

That is: keep the anchor line and the `#[async_trait::async_trait]` attribute byte-for-byte, and
insert the twelve `///` lines between them. Lines 1-12 of the file and everything from
`pub trait SettingsStore: Send + Sync {` downwards are untouched. The file goes from **139** to
**151** lines.

### 6.2 Why every clause of it is true (verified, not asserted)

| Clause | Evidence in the current tree |
| --- | --- |
| `with_lock` is async only because `FileLock::acquire` is | `store.rs:69` is the only `.await` in `FileSettingsStore::with_lock`; `InMemorySettingsStore::with_lock` (`:122-138`) has no `.await` at all and is `async` solely to satisfy the trait |
| that lock awaits an in-process keyed mutex, then a non-blocking `flock` retry loop | `lock.rs:20-24` (`CONFIG_LOCKS` / `CONFIG_LOCK_HANDLE`), `lock.rs:50-73` (the `FileLock` type doc: layer 1 keyed mutex, layer 2 `LOCK_EX\|LOCK_NB` retried from async land) |
| `FileSettingsStore::read` is a bare `std::fs::read_to_string` | `store.rs:51-61` |
| `InMemorySettingsStore`'s is a `std::sync::Mutex` lock plus a clone | `store.rs:118-120` |
| `read`'s only caller is `SettingsManager::load_scope` | `self.store.read(scope)` at `manager.rs:78` is the sole non-test call of the trait method; the only other hits are two assertions in `cyrup/src/startup.rs:387/389`, inside `#[cfg(test)] mod tests` (`:336`) |
| under `reload_internal`, under sync `load` / `reload` / `set_project_trusted` | `load_scope` (`manager.rs:77`) is called only from `reload_internal` (`:94`), which is called from `pub fn load` (`:47`, via `mgr.reload_internal()` at `:58`), `pub fn reload` (`:139`) and `pub fn set_project_trusted` (`:174`) — all three still `fn`, not `async fn` |
| `lockfile.lockSync` behind a busy-wait retry loop | pi `settings-manager.ts:206`, inside `acquireLockSyncWithRetry` (`:199-224`), 10 attempts × 20 ms |
| the annotation *"Sleep synchronously to avoid changing callers to async"* | pi `settings-manager.ts:218` |
| `readFileSync` | pi `settings-manager.ts:237` |
| `@v0.83.0` | `tmp/pi/packages/coding-agent/package.json` → `"version": "0.83.0"` |
| both intra-doc links resolve | `pub mod lock;` at `cyrup-config/src/lib.rs:27`, `pub struct FileLock` and `pub async fn acquire` in it; `InMemorySettingsStore` is `pub` in the same module (`store.rs:92`) |
| the `**[CYRUP-DELTA]**` spelling matches the crate | `model/validate.rs:246`, `model/schema.rs:13`, `provider_compose.rs:169` |

### 6.3 Formatting facts, already checked

- Longest added line is **99 characters**; the file's pre-existing longest is 99 (`store.rs:1`).
  The workspace has **no** `rustfmt.toml`, so `max_width = 100` and `wrap_comments = false` apply —
  rustfmt will not reflow these lines.
- The post-edit file was assembled in a scratch copy and `rustfmt --check --edition 2024` on it
  printed nothing.

### 6.4 Ordering with the sibling task

Both edits are in `store.rs` but in disjoint regions — this one strictly above
`#[async_trait::async_trait]`, that one on the `with_lock` method rustdoc and inside the two impl
bodies. **Whichever lands second must re-anchor by content**, using the `grep -c` assertion in §6.1
rather than the line numbers quoted in its own file. Either order works. Note for whoever reads the
sibling's checklist afterwards: its `wc -l … → 141` expectation assumes this hunk is absent; with
both applied the file is **153** lines.

---

## 7. Decided here, explicitly

The sibling file routes two further questions to this task. Both are answered **no**, with the
evidence, so neither is re-derived later.

### 7.1 Should `read` also become `async`? **No.**

Four independent reasons, each verified against the tree:

1. **There is nothing to await.** [`FileSettingsStore::read`](../../crates/cyrup-config/src/settings/store.rs)
   (`store.rs:51-61`) is a bare `std::fs::read_to_string`; `InMemorySettingsStore::read`
   (`:118-120`) is a `std::sync::Mutex` lock and a `clone`. Neither acquires a `FileLock`. `async`
   would produce a future that completes on its first poll, boxed by `async_trait` on every call.
2. **It would break three more public methods for that.** `read`'s only caller is the private
   `SettingsManager::load_scope` (`manager.rs:77-78`), reached only from `reload_internal` (`:94`),
   reached from `pub fn load` (`:47`), `pub fn reload` (`:139`) and `pub fn set_project_trusted`
   (`:174`) — all sync today. `SettingsManager::load` alone has **8 non-test call sites**, and
   `set_project_trusted` one:

   | Call site | |
   | --- | --- |
   | `cyrup/src/bootstrap.rs:62`, `:98`, `:215`, `:267` | `SettingsManager::load` |
   | `cyrup/src/prelaunch.rs:268` | `SettingsManager::load` |
   | `cyrup/src/startup_ui.rs:47` | `SettingsManager::load` |
   | `cyrup/src/subcommands.rs:814`, `:837` | `SettingsManager::load` |
   | `cyrup-session-svc/src/builder.rs:689` | `SettingsManager::set_project_trusted` |

   (`cyrup/src/startup.rs:423` and `:440` also call `load` but sit inside `#[cfg(test)] mod tests`
   at `:336`, as does `cyrup/tests/first_time_setup.rs:372` and the `cyrup-config` unit tests.)
3. **Upstream is sync on both halves, and says why.** pi's `SettingsStorage` interface
   (`settings-manager.ts:179-181` @v0.83.0) declares **only** `withLock`; the file read is
   `readFileSync` (`:237`) and the lock is `lockfile.lockSync` (`:206`) behind a *synchronous*
   busy-wait retry loop whose own comment is *"Sleep synchronously to avoid changing callers to
   async"* (`:218`). cyrup's `async` on `with_lock` is a deliberate mechanism difference: it buys
   a cancel-aware in-process queue and a non-blocking `flock` retry instead of a 10 × 20 ms
   synchronous poll. `read` buys nothing by diverging.
4. **Symmetry is not a reason.** The asymmetry is load-bearing information: it tells the reader
   exactly which half touches the lock.

### 7.2 Should any of the four get a deprecation shim or softening re-export? **No.**

| Item | Why no shim |
| --- | --- |
| `FileLock::acquire` | A 1-arg shim must choose a `CancelToken` for the caller. The correct choice is `NEVER_CANCELLED` (`lock.rs:34`), which is what `None` already means (`lock.rs:105`, `cancel.unwrap_or(&NEVER_CANCELLED)`) — the shim would be a rename of `None`, and it would freeze a signature two open tasks are actively reworking. |
| `SettingsStore::with_lock` | A default method or blanket impl cannot bridge sync→async on a `dyn`-consumed trait without either boxing every call or silently blocking the runtime. Two impls exist, both in `store.rs`, both converted. |
| `TrustPromptFn` | [`MEDIUM-trustpromptfn-…`](MEDIUM-trustpromptfn-public-break-and-its-misstated-cost.md) §2 closed the question: keep the alias, fix the doc. A parallel `TrustPromptFnSync` would need the builder to persist the chosen option itself, which is a *larger* break and a divergence from pi's SEAM-065. |
| `MutationGuard` | Nothing outside `cyrup-tools/src/lock.rs` names the type — `grep -rnw MutationGuard crates/ --include=*.rs` returns exactly two hits, the declaration (`:85`) and the return type of `guard` (`:144`). Every other `…MutationGuard…` hit in the workspace is `CompletionMutationGuardResult`, an unrelated `cyrup-ext-subagents` type. A shim would preserve compatibility nothing is consuming. |

---

## 8. Out of scope for this task

- **Any source file other than `crates/cyrup-config/src/settings/store.rs`.**
- The `with_lock` **method** rustdoc, the `for<'s>`/`+ Send` explanation, the `let next` bindings
  and the two impl-body comments — all
  [`MEDIUM-with-lock-…`](MEDIUM-with-lock-let-next-comments-invent-a-borrowck-rule.md)'s.
- The prescriptions in
  [`MEDIUM-trustpromptfn-…`](MEDIUM-trustpromptfn-public-break-and-its-misstated-cost.md),
  [`LOW-mutationguard-…`](LOW-mutationguard-alias-erases-the-lock-domain-type-distinction.md),
  [`LOW-keyed-lock-map-alias-exposes-the-raw-dashmap.md`](LOW-keyed-lock-map-alias-exposes-the-raw-dashmap.md),
  [`MEDIUM-cancel-token-…`](MEDIUM-cancel-token-does-not-reach-the-cross-process-flock-wait.md) and
  [`HIGH-dropped-acquire-…`](HIGH-dropped-acquire-future-detaches-blocking-flock-task.md). Do not
  restate or pre-empt them; §2 links, it does not duplicate.
- Creating `CHANGELOG.md`, `docs/MIGRATION.md`, release notes, a book page or a new ADR. Closed by
  §5. The twelve `///` lines in §6.1 are the entire documentation footprint of this task.
- Reverting or redesigning any of the 28. Closed by §7 and by the four dedicated tasks.
- Adding `#[deprecated]` anywhere. Nothing is deprecated; the old spellings do not exist.
- **Tests and benchmarks.** Nothing behavioural changes — the hunk is a doc comment. Do not add,
  rename, or run tests for this task; a different team owns the suite.
- A workspace-wide or crate-wide `cargo fmt`. `store.rs` is rustfmt-clean before and after.

---

## 9. Definition of done

All checks are read-only inspections of the working tree plus one compile. **No git command is
required or permitted, and no test is written or run.** Run each from the repo root.

1. `wc -l crates/cyrup-config/src/settings/store.rs` → **151** (was 139; +12, all `///`).
2. `grep -c '^\s*///' crates/cyrup-config/src/settings/store.rs` → **21** (was 9), and
   `grep -vc '^\s*///' crates/cyrup-config/src/settings/store.rs` → **130**, unchanged. Every
   added line is a doc line; no non-doc line moved, appeared or disappeared.
3. `grep -n 'Do not "finish the job"' crates/cyrup-config/src/settings/store.rs` → exactly **one**
   hit, and its line number is **less than** the line number of
   `grep -n '^#\[async_trait::async_trait\]' … | head -1` (i.e. the block is the *trait's* rustdoc,
   not `with_lock`'s and not an impl's).
4. `sed -n '9,26p' crates/cyrup-config/src/settings/store.rs` reproduces, in this order: the
   existing `Serialized read-modify-write…` line, `///`, the two existing `#[async_trait]`-rationale
   lines, `///`, the twelve new lines exactly as written in §6.1, then
   `#[async_trait::async_trait]`, then `pub trait SettingsStore: Send + Sync {`.
5. `grep -c '\*\*\[CYRUP-DELTA\]\*\*' crates/cyrup-config/src/settings/store.rs` → **1**.
6. `awk 'length($0) > 100 {print FILENAME":"FNR" "length($0)}' crates/cyrup-config/src/settings/store.rs`
   → no output.
7. `rustfmt --check --edition 2024 crates/cyrup-config/src/settings/store.rs` prints nothing.
8. `cargo check -p cyrup-config` succeeds with no new warnings.
9. Both intra-doc link targets still exist, checked by name, not by line:
   `grep -q '^pub mod lock;' crates/cyrup-config/src/lib.rs`,
   `grep -q 'pub async fn acquire' crates/cyrup-config/src/lock.rs`, and
   `grep -q 'pub struct InMemorySettingsStore' crates/cyrup-config/src/settings/store.rs` all
   succeed.
10. `crates/cyrup-config/src/settings/store.rs` is the **only** file modified, and **no new file
    exists anywhere in the repo as a result of this task** — in particular no `CHANGELOG.md`, no
    migration note, no new ADR. Confirm all four:
    `find . -iname 'CHANGELOG*' -not -path './target/*' -not -path './tmp/*'` → no output;
    `[ ! -e docs/MIGRATION.md ]` → true; `ls docs/adr | wc -l` → **13** (11 ADRs + `README.md` +
    `LEADS-SETTLED.md`, unchanged); and `ls crates/cyrup-config/src/settings/` lists exactly the
    modules it listed before — this task adds no module.
11. This file's `status:` is `done`, and §0-§4 are left intact: they, not a changelog, are the
    record of the 28.

---

## 10. QA addendum (2026-08-23, review-only)

The §6.1 hunk is present in `crates/cyrup-config/src/settings/store.rs` at lines 13-24,
byte-for-byte as specified, immediately above `#[async_trait::async_trait]` (`:25`). Verified
against the real tree and the vendored pi source:

- `FileLock::acquire` awaits layer 1 (keyed async mutex, `lock.rs:19-23`/`:125-129`) then a
  non-blocking `LOCK_EX|LOCK_NB` retry loop — as stated.
- `store.rs:89` is the only `.await` in `FileSettingsStore::with_lock`; `InMemorySettingsStore::with_lock`
  has none.
- `FileSettingsStore::read` (`:71-81`) is `std::fs::read_to_string`; `InMemorySettingsStore::read`
  (`:135-137`) is a `std::sync::Mutex` lock plus a clone.
- `self.store.read(scope)` at `manager.rs:78` is the only non-test call; `load_scope` (`:77`) is
  reached only from `reload_internal` (`:100`, `:102`), whose public sync ancestors `load` (`:47`),
  `reload` (`:139`) and `set_project_trusted` (`:174`) are all still `pub fn`.
- pi `settings-manager.ts`: `lockfile.lockSync` at `:206`, the *"Sleep synchronously to avoid
  changing callers to async"* comment at `:218`, `readFileSync` at `:237`, `SettingsStorage`
  declaring only `withLock` at `:179-181`; `package.json` → `"version": "0.83.0"`. All exact.
- `rustfmt --check --edition 2024` clean; no line exceeds 100 chars; `cargo check -p cyrup-config`
  clean; `cargo doc -p cyrup-config --no-deps` with `-D rustdoc::broken_intra_doc_links` clean, so
  both intra-doc links resolve.
- No `CHANGELOG*`, no `docs/MIGRATION.md`, `docs/adr` still 13 entries, no new settings module.

**Counts differ from §9 because the sibling hunk landed too**, exactly as §6.4 predicted: the file
is **153** lines (not 151), **29** doc lines and **124** non-doc — `MEDIUM-with-lock-…` added 8 doc
lines to the `with_lock` method rustdoc and collapsed the 6-line `let next` bindings. Nothing in
this task's region moved.

**Known post-hoc drift in the §1/§2 record, left as-is.** `LOW-mutationguard-alias-erases-the-lock-domain-type-distinction`
has since landed and restored the newtype: `cyrup-tools/src/lock.rs:114` now reads
`pub struct MutationGuard(KeyedGuard<PathBuf>)`, and `FileMutationLocks::guard` (`:172-182`) ends
`.map(MutationGuard).map_err(|_| error::aborted())`. §1 bucket (c)/(d) and the §2 rows describe the
branch state the review enumerated, not today's tree; the Owner links carry the resolution. Do not
"correct" them into a claim about the current tree — treat §0-§4 as a dated snapshot.

Nits, not defects, recorded so they are not re-litigated: *"`read` takes no lock in either impl"*
sits in the same sentence as *"a `std::sync::Mutex` lock"* — the referent is the cross-process
`FileLock`, and the operative conclusion (no suspension point) holds for both impls; and *"`read`'s
only caller"* means the only non-test caller (`manager.rs:78`), the test suites call it directly.
