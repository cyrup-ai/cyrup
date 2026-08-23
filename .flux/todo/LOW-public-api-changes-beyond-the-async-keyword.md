---
title: Public Api Changes Beyond The Async Keyword
priority: LOW
stage: aug
status: done
updated: 2026-08-23 03:44
---

# The 28-item public-surface record — and the one line of it that is still an open change

## Verdict

The reviewer's enumeration is **correct in its counts and in its four callouts**, and re-derived
below from the diff rather than taken on trust. It is an inventory, not a defect, so the first
question is what the deliverable is. Three candidates were considered:

| Candidate | Decision | Why |
| --- | --- | --- |
| (a) A CHANGELOG / release-note entry somewhere in the repo | **Rejected** | The workspace has no changelog, no git tags, no `.github/`, no release pipeline, and every crate is `version = "0.0.0"`. `README.md:9` states *"not a released product."* There is no convention to add to, and a LOW inventory task is the wrong place to invent one — see §5. |
| (b) A deprecation shim or softening re-export for some of the four | **Rejected** | Every consumer of all four is in-workspace and named in §2. A shim would have to pick a `CancelToken` on the caller's behalf (`FileLock::acquire`), or preserve a nominal type the other two tasks have already ruled correct as-is. See §7. |
| (c) Purely informational — close as recorded | **Adopted, with one exception** | The record IS the deliverable. **The exception**: [`MEDIUM-with-lock-let-next-comments-invent-a-borrowck-rule.md`](MEDIUM-with-lock-let-next-comments-invent-a-borrowck-rule.md) explicitly hands this file two things it declined to do — the *"why did `read` stay sync"* sentence, and the decision on whether `read` should become async at all. Those are answered and specified in §6 and §7. |

**Required path: the single rustdoc hunk in §6.** One file, one doc-comment block, zero non-comment
lines. Nothing else in this task changes any source file.

---

## 1. The count, re-derived

Command basis: `git diff 4902cddf8ce7d4723e41b4a7bf652361a584f905...HEAD -- crates/`, reading every
`+`/`-` line that declares an item, with `#[cfg(test)]` bodies excluded from the public tally.

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

Everything else in the diff that touches a `pub` line is either a call site gaining `.await`, or
rustfmt reflow (`ops/shell.rs`, `config.rs`) that
[`LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md`](LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md)
owns. Verified individually, no exceptions found.

**A negative result worth recording, because it is the largest single hunk in the diff.** The
`ops/local.rs` → `ops/local/` decomposition (−1912/+1279 lines across seven new files) moves
**zero** public API. Every submodule is `pub(crate)`
([`ops/local/mod.rs:25-30`](../../crates/cyrup-tools/src/ops/local/mod.rs)) and every previously
public item is re-exported at its original path (`LocalFs`, `LocalProc`, `kill_pid`,
`kill_process_tree`, `terminate_pid`, `kill_tracked_detached_children`, `track_detached_child_pid`,
`untrack_detached_child_pid`), with
[`ops/mod.rs:17-21`](../../crates/cyrup-tools/src/ops/mod.rs) byte-identical to merge base apart
from rustfmt's import reordering. `cyrup_tools::ops::local::LocalFs` still resolves. A reader
skimming the diff by line count would guess the opposite.

---

## 2. The five non-mechanical changes

### (b) Parameter added — breaking

| Item | Merge base → HEAD |
| --- | --- |
| [`cyrup_config::lock::FileLock::acquire`](../../crates/cyrup-config/src/lock.rs) (`lock.rs:57`) | `pub fn acquire(target: &Path) -> Result<Self, ConfigError>` → `pub async fn acquire(target: &Path, cancel: Option<&CancelToken>) -> Result<Self, ConfigError>` |

Every out-of-tree caller must pass a second argument. In-tree, six call sites; all pass `None`
except `models_store`'s three. The semantics of that parameter — and the fact that it does not
reach the `flock` wait — belong to
[`MEDIUM-cancel-token-does-not-reach-the-cross-process-flock-wait.md`](MEDIUM-cancel-token-does-not-reach-the-cross-process-flock-wait.md)
and [`HIGH-dropped-acquire-future-detaches-blocking-flock-task.md`](HIGH-dropped-acquire-future-detaches-blocking-flock-task.md).

### (c) Type changed — breaking

| Item | Merge base → HEAD | Owner |
| --- | --- | --- |
| [`cyrup_config::settings::SettingsStore::with_lock`](../../crates/cyrup-config/src/settings/store.rs) (`store.rs:20-24`) | `fn with_lock(&self, scope, f: &mut dyn FnMut(Option<&str>) -> Option<String>)` → `async fn` on an `#[async_trait]` trait, `f: &mut (dyn for<'s> FnMut(Option<&'s str>) -> Option<String> + Send)` | §6 here, plus [`MEDIUM-with-lock-…`](MEDIUM-with-lock-let-next-comments-invent-a-borrowck-rule.md) |
| [`cyrup_session_svc::TrustPromptFn`](../../crates/cyrup-session-svc/src/builder.rs) (`builder.rs:438-445`) | returns `Option<bool>` → returns `Pin<Box<dyn Future<Output = Option<bool>> + Send + 'a>>` under a new `for<'a>` binder | [`MEDIUM-trustpromptfn-public-break-and-its-misstated-cost.md`](MEDIUM-trustpromptfn-public-break-and-its-misstated-cost.md) |
| [`cyrup_tools::lock::MutationGuard`](../../crates/cyrup-tools/src/lock.rs) (`lock.rs:85`) | `pub struct MutationGuard { … }` → `pub type MutationGuard = KeyedGuard<PathBuf>` | [`LOW-mutationguard-alias-erases-the-lock-domain-type-distinction.md`](LOW-mutationguard-alias-erases-the-lock-domain-type-distinction.md) |

`SettingsStore` is the one that changes three things at once, and an external implementor gets all
three compiler errors together: the trait now needs the `async_trait` proc macro to implement, the
closure parameter gained `Send`, and it gained an explicit higher-ranked lifetime. It is consumed
as `Arc<dyn SettingsStore>` in four crates —
[`cyrup-config/src/settings/manager.rs:26`](../../crates/cyrup-config/src/settings/manager.rs),
[`cyrup-session-svc/src/builder.rs:377`](../../crates/cyrup-session-svc/src/builder.rs) and
[`factory.rs:27`](../../crates/cyrup-session-svc/src/factory.rs), and
[`cyrup/src/startup.rs:299`](../../crates/cyrup/src/startup.rs).

### (d) Same declaration, different return type

| Item | What actually changed |
| --- | --- |
| [`cyrup_tools::lock::FileMutationLocks::guard`](../../crates/cyrup-tools/src/lock.rs) (`lock.rs:140-146`) | Signature text is **byte-identical** to merge base: `pub async fn guard(&self, path: &Path, cancel: &CancelToken) -> Result<MutationGuard, ToolError>`. But `MutationGuard` is now `KeyedGuard<PathBuf>`, so the returned type is a different, foreign, generic type. The body shrank from ~40 lines to one delegation, `self.inner.guard(key, cancel).await.map_err(…)`. |

This is the item most likely to be mis-triaged by a future reader: `git diff` shows no change to the
line, so a signature-level audit reports "unchanged", and only following the alias reveals it. It is
recorded here for exactly that reason; the consequences are
[`LOW-mutationguard-…`](LOW-mutationguard-alias-erases-the-lock-domain-type-distinction.md)'s.

---

## 3. The 23 mechanical ones — `async` keyword, nothing else

Each verified as a one-token `fn` → `async fn` change, with the parameter list, return type and
generics byte-identical to merge base.

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
carried the lifetime at merge base (`management.rs:1337`), and the `<'_>` is explicit elision the
`async` conversion made the compiler ask for. Confirmed by reading both revisions of the
declaration.

**Private signature changes**, listed only so the (b) audit is provably complete — none is `pub`:

- [`FileModelsStore::read_latest`](../../crates/cyrup-config/src/models_store.rs) (`:234`) gained
  `options: Option<&ModelsStoreOperationOptions>` and became `async`. It is a private inherent
  method on a `pub struct`; the rationale in its doc is
  [`LOW-models-store-rationale-still-rests-on-a-sync-read-latest.md`](LOW-models-store-rationale-still-rests-on-a-sync-read-latest.md)'s.
- `lock_settings_file` (`settings_write.rs`), `saved_trusted`
  ([`cyrup/src/subcommands.rs`](../../crates/cyrup/src/subcommands.rs)) and `persist_trust_choice`
  ([`cyrup/src/startup_ui.rs`](../../crates/cyrup/src/startup_ui.rs)) became `async`.
- `handle_disable` / `handle_enable` / `handle_reset` (`management.rs`) became `async` and picked up
  the same `<'_>` elision as their public caller.

---

## 4. Additive surface — new, and it does reach the embedder crate

[`cyrup-core/src/lib.rs`](../../crates/cyrup-core/src/lib.rs) gains `pub mod keyed_lock;` (`:16`)
and root re-exports of `Cancelled`, `KeyedGuard`, `KeyedLockMap`, `KeyedLocks` (`:33`). Both
`cyrup-core` and `cyrup-config` take a new `dashmap` dependency.

This is the **only** part of the diff that lands on the ADR-0008 embedder surface, because
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
| `SettingsStore`, `SettingsManager`, `TrustStore`, `FileLock` | **no** | `cyrup-sdk` has no `cyrup-config` dependency, and ADR-0008 §C rules that re-export out on purpose |
| `MutationGuard`, `FileMutationLocks` | **not yet** | `cyrup-sdk` has no `cyrup-tools` dependency **today**, but ADR-0008 §C files a task to add one and re-export *"the file lock/mutation-queue primitive"* (`sdk.ts:114-126`). The alias change lands on a type already scheduled to become embedder-visible. |

---

## 5. What this repo does with a breaking change — and why the answer is "nothing"

Searched, not assumed:

- **No `CHANGELOG`** anywhere: `find . -iname 'CHANGELOG*'` (excluding `target/`) returns nothing.
- **No git tags**: `git tag` is empty. **No `.github/`** directory, so no release workflow.
- **No versions**: `[workspace.package] version = "0.0.0"`
  ([`Cargo.toml:87`](../../Cargo.toml)); no crate overrides it. Only `cyrup-it` and
  `cyrup-test-support` set `publish = false`, and `cyrup-it/Cargo.toml:22` says of that flag *"it is
  a registry flag and has no effect whatsoever"* — i.e. the repo already treats registry semantics
  as inapplicable.
- **`README.md:9`**: *"**Status:** not a released product."*
- **No `docs/` migration file.** `docs/` holds `adr/` (11 decision records), `gap-analysis/`
  (upstream-parity ledger), `audits/`, `guide/` (the mdBook, `book.toml`), `PARITY-PLAN.md`,
  `TEST-ARCHITECTURE.md`. None is a change log; `gap-analysis/` tracks divergence from **pi**, not
  from cyrup's own past.
- **The one convention that does exist for "a reader will be surprised by this"** is an in-source
  `[CYRUP-DELTA]` comment naming the upstream line and the reason (421 occurrences under `crates/`,
  the policy stated at `README.md:26-28`). It is for pi-vs-Rust mechanism differences, which is a
  different axis from API stability — but it is why §6's deliverable is a **rustdoc comment** and
  not a file: in this repo, the place a non-obvious mechanism choice gets recorded is next to the
  declaration that made it.

**The one public-surface policy that exists is [ADR-0008](../../docs/adr/ADR-0008-requirement-ids-and-sdk-surface.md)
§C**, and all 28 items still satisfy it. Its rule is a *parity* rule — every capability pi exports
*"must exist, be `pub`, and be reachable from a published workspace crate"* — not a stability rule.
Nothing here was removed, un-`pub`'d, or made unreachable, so nothing trips it. Worth stating
plainly, because "28 public signatures moved" reads like an ADR-0008 event and is not one.

**Conclusion: creating `CHANGELOG.md` would be inventing a repo-wide convention from inside a LOW
inventory task, for a workspace with zero releases and zero out-of-tree consumers. Do not.** If a
maintainer later wants one, it is a repo-level decision belonging in `docs/adr/`, seeded with more
than this branch. This file is the record for this branch, and it is checked in
(`.flux/README.md`: *"the queue travels with the repo, so it is visible in review, survives a fresh
clone"*), which is the property a changelog entry would have bought.

---

## 6. The one required change

One file, one hunk: the **trait-level** rustdoc of `SettingsStore`,
[`crates/cyrup-config/src/settings/store.rs:9-12`](../../crates/cyrup-config/src/settings/store.rs).

[`MEDIUM-with-lock-…`](MEDIUM-with-lock-let-next-comments-invent-a-borrowck-rule.md) lists that
paragraph under **Do not touch** and assigns it here verbatim: *"The 'why did `read` stay sync'
sentence belongs to `LOW-public-api-changes-beyond-the-async-keyword.md`, not here."* That task owns
the `with_lock` **method** rustdoc (its hunk 1) and both impl bodies; this task owns the **trait**
paragraph above them. The two do not overlap.

The gap: the existing paragraph explains why `#[async_trait]` was chosen over a native `async fn`,
but not why the trait ends up **half** async — `read` sync, `with_lock` not. A contributor reading
only the declaration sees an inconsistency with no stated reason, and the obvious "fix" is to box
`read` too.

**Append after `store.rs:12`** (the line ending `…and a native async fn is not dyn-compatible.`),
leaving lines 1-12 and the `#[async_trait::async_trait]` attribute at `:13` untouched:

```rust
///
/// The trait is HALF async on purpose. `with_lock` is `async` only because
/// [`crate::lock::FileLock::acquire`] is — that lock now awaits an in-process mutex and then a
/// `spawn_blocking` `flock`. `read` takes no lock in either impl (`std::fs::read_to_string`,
/// and a `std::sync::Mutex` clone in [`InMemorySettingsStore`]), so it has no suspension point
/// at all, and `async` there would box a future that never yields. It would also cascade:
/// `read`'s only caller is `SettingsManager::load_scope` (`manager.rs:78`), under
/// `reload_internal`, under the sync `pub fn load` / `reload` / `set_project_trusted`.
/// Upstream is sync on BOTH halves — `lockfile.lockSync` behind a busy-wait retry loop
/// annotated "Sleep synchronously to avoid changing callers to async"
/// (settings-manager.ts:206/218 @v0.83.0), and `readFileSync` at `:237` — so the `async` here
/// is a mechanism difference confined to the half that takes a lock. Do not "finish the job".
```

Every line is ≤ 100 columns, matching the file. `[`crate::lock::FileLock::acquire`]` follows the
established intra-doc style in this crate (`models_store.rs:7`, `keybindings.rs:312`,
`error.rs:78`); `[`InMemorySettingsStore`]` is same-module.

**Ordering with the sibling task.** Both edits are in `store.rs` but in disjoint regions — this
one strictly above `:13`, that one at `:18-24` and inside the two impl bodies. Whichever lands second
must re-anchor by content, not by the line numbers quoted in its own file. Either order works.

---

## 7. Decided here, explicitly

The same sibling file routes two further questions to this task. Both are answered **no**, with the
evidence, so neither is re-derived later.

### 7.1 Should `read` also become `async`? **No.**

Four independent reasons, each verified against the tree:

1. **There is nothing to await.** [`FileSettingsStore::read`](../../crates/cyrup-config/src/settings/store.rs)
   (`:51-61`) is a bare `std::fs::read_to_string`; `InMemorySettingsStore::read` (`:118-120`) is a
   `std::sync::Mutex` lock and a `clone`. Neither acquires a `FileLock`. `async` would produce a
   future that completes on its first poll, boxed by `async_trait` on every call.
2. **It would break three more public methods for that.** `read`'s only caller is the private
   `SettingsManager::load_scope` (`manager.rs:77-78`), reached only from `reload_internal` (`:94`),
   reached from `pub fn load` (`:47`), `pub fn reload` (`:139`) and `pub fn set_project_trusted`
   (`:174`) — all sync today. `load` alone has **8 non-test call sites** (`main.rs:176`, `:343`,
   `:551`, `:601`, `:1568`; `startup_ui.rs:47`; `subcommands.rs:814`, `:837`), and
   `set_project_trusted` one (`builder.rs:689`).
3. **Upstream is sync on both halves, and says why.** pi's `SettingsStorage` interface
   (`settings-manager.ts:179-181` @v0.83.0) declares **only** `withLock`; the file read is
   `readFileSync` (`:237`) and the lock is `lockfile.lockSync` (`:206`) behind a *synchronous*
   busy-wait retry loop whose own comment is *"Sleep synchronously to avoid changing callers to
   async"* (`:218`). cyrup's `async` on `with_lock` is a deliberate mechanism difference: it buys
   a cancel-aware in-process queue and a blocking-pool `flock` instead of a 10 × 20 ms poll. `read`
   buys nothing by diverging.
4. **Symmetry is not a reason.** The asymmetry is load-bearing information: it tells the reader
   exactly which half touches the lock.

### 7.2 Should any of the four get a deprecation shim or softening re-export? **No.**

| Item | Why no shim |
| --- | --- |
| `FileLock::acquire` | A 1-arg shim must choose a `CancelToken` for the caller. The correct choice is `NEVER_CANCELLED` (`lock.rs:29`), which is what `None` already means — the shim would be a rename of `None`, and it would freeze a signature two open tasks are actively reworking. |
| `SettingsStore::with_lock` | A default method or blanket impl cannot bridge sync→async on a `dyn`-consumed trait without either boxing every call or silently blocking the runtime. Two impls exist, both in-tree, both converted. |
| `TrustPromptFn` | [`MEDIUM-trustpromptfn-…`](MEDIUM-trustpromptfn-public-break-and-its-misstated-cost.md) §2 compiled both alternative shapes and closed the question: keep the alias, fix the doc. A parallel `TrustPromptFnSync` would need the builder to persist the chosen option itself, which is a *larger* break and a divergence from pi's SEAM-065. |
| `MutationGuard` | Nothing outside `cyrup-tools/src/lock.rs` names the type — verified by grep in [`LOW-mutationguard-…`](LOW-mutationguard-alias-erases-the-lock-domain-type-distinction.md). A shim would preserve compatibility nothing is consuming. |

---

## 8. Out of scope for this task

- **Any source file other than `crates/cyrup-config/src/settings/store.rs`.**
- The `with_lock` **method** rustdoc, the `for<'s>`/`+ Send` explanation, the `let next` bindings
  and the two impl-body comments — all
  [`MEDIUM-with-lock-…`](MEDIUM-with-lock-let-next-comments-invent-a-borrowck-rule.md)'s hunks 1-3.
- The prescriptions in
  [`MEDIUM-trustpromptfn-…`](MEDIUM-trustpromptfn-public-break-and-its-misstated-cost.md),
  [`LOW-mutationguard-…`](LOW-mutationguard-alias-erases-the-lock-domain-type-distinction.md),
  [`LOW-keyed-lock-map-alias-exposes-the-raw-dashmap.md`](LOW-keyed-lock-map-alias-exposes-the-raw-dashmap.md),
  [`MEDIUM-cancel-token-…`](MEDIUM-cancel-token-does-not-reach-the-cross-process-flock-wait.md) and
  [`HIGH-dropped-acquire-…`](HIGH-dropped-acquire-future-detaches-blocking-flock-task.md). Do not
  restate or pre-empt them; §2 links, it does not duplicate.
- Creating `CHANGELOG.md`, `docs/MIGRATION.md`, release notes, or a new ADR. Closed by §5.
- Reverting or redesigning any of the 28. Closed by §7 and by the four dedicated tasks.
- Adding `#[deprecated]` anywhere. Nothing is deprecated; the old spellings do not exist.
- Tests. Nothing behavioural changes — the hunk is a doc comment.
- A workspace-wide `cargo fmt`. `store.rs` is already rustfmt-clean and every added line is ≤ 100
  columns.

---

## 9. Definition of done

1. `crates/cyrup-config/src/settings/store.rs` is the **only** file this task changes, and
   `git diff` for it shows **zero** non-comment `+`/`-` lines. Every added line begins with `///`.
2. The added block sits **above** `#[async_trait::async_trait]` on the trait (i.e. it is the trait's
   rustdoc, not `with_lock`'s), directly after the existing `…not dyn-compatible.` line, separated
   from it by a bare `///`.
3. The doc states, in this order: that `with_lock` is async **only** because `FileLock::acquire` is;
   that `read` takes no lock and therefore has no suspension point; that making it async would
   cascade to `load` / `reload` / `set_project_trusted`; and that upstream is sync on both halves.
4. The string `Do not "finish the job"` (or equivalent explicit instruction not to box `read`)
   appears exactly once in the file.
5. No line in the added block exceeds 100 columns.
6. `rustfmt --check --edition 2024 crates/cyrup-config/src/settings/store.rs` is silent.
7. `cargo doc -p cyrup-config` resolves `[`crate::lock::FileLock::acquire`]` and
   `[`InMemorySettingsStore`]` with no broken-intra-doc-link warning.
8. **No new file exists anywhere in the repo as a result of this task** — in particular no
   `CHANGELOG.md`, no `docs/` migration note. `git status --short` shows `store.rs` and this task
   file, nothing else.
9. This file's `status:` is `done`, and §1-§4 are left intact: they, not a changelog, are the
   record of the 28.
