---
title: Config Lock Skips The Path Resolution Its Own Module Contract Requires
priority: LOW
stage: aug
status: done
updated: 2026-08-23 03:44
---

# Make `cyrup-config`'s lock key canonical for its own key space, and correct the contract that says both domains resolve the same way

OBJECTIVE: layer 1 of [`cyrup-config`'s `FileLock`](../../crates/cyrup-config/src/lock.rs) must key
on a **lexically absolute** sidecar path, and the two docs that assert "one task per path per
process" as a flat fact must state what it actually guarantees — including the one alias it does not
cover, and why that residue is harmless *here* and would not be upstream.

This is a small change (one key helper, one `pub(crate)`, three doc blocks). Most of what follows is
the research that fixes the required path, because the review's suggested fix ("resolve, matching
`cyrup-tools`") is the **wrong** resolution for this domain and the evidence for that is not in
either lock file.

---

## 1. What the review got right

The mismatch is real. [`cyrup-core/src/keyed_lock.rs:8-10`](../../crates/cyrup-core/src/keyed_lock.rs)
states a flat obligation:

> Keying is the caller's job too. This module never touches the filesystem (see the crate docs:
> no I/O), so a domain that keys on resolved paths resolves them itself before calling
> [`KeyedLocks::guard`].

[`cyrup-tools/src/lock.rs:118-126`](../../crates/cyrup-tools/src/lock.rs) honours it with
`tokio::fs::canonicalize` + an ENOENT/ENOTDIR fallback.
[`cyrup-config/src/lock.rs:57-63`](../../crates/cyrup-config/src/lock.rs) does not — it hands
`lock_path_for(target)` straight to `guard`, and
[`lock_path_for` (`:108-118`)](../../crates/cyrup-config/src/lock.rs) is a pure string transform
(`parent().join(file_name + ".lock")`) with no resolution of any kind.

And the invariant that rests on it is asserted in **three** places, not one:

- [`lock.rs:35-41`](../../crates/cyrup-config/src/lock.rs) — "at most one task per path per process,
  so the cross-process `flock` has exactly one waiter", which is the stated licence for running a
  *blocking* `flock` on the blocking pool;
- [`auth.rs:269-272`](../../crates/cyrup-config/src/auth.rs) — "`FileLock`'s in-process layer keys
  per **FILE**", used to argue the file lock subsumes the per-provider mutex;
- the HIGH sibling finding's argument (b) quotes `lock.rs:35-41` verbatim as the thing it violates.

## 2. What the review got wrong — corrections that change the fix

**(a) The two `settings.json` files are not the same file.** The review says
`FileSettingsStore.{global_path, project_path}` and `cyrup-ext-subagents`'
`user_settings_path` are "two separately constructed spellings that, on a default layout, name the
same `settings.json`". They are not, on any layout that isn't hand-made:

| producer | default path |
| --- | --- |
| [`ConfigDirs::settings_path()`](../../crates/cyrup-config/src/env.rs) (`env.rs:311`) | `<agent_dir>/settings.json` = `~/.cyrup/**agent**/settings.json` |
| [`ConfigDirs::project_settings_path()`](../../crates/cyrup-config/src/env.rs) (`env.rs:323`) | `<cwd>/.cyrup/settings.json` |
| [`resolve.rs:112`](../../crates/cyrup-ext-subagents/src/extension/executor/resolve.rs) | `dirs_home()/.cyrup/**agents**/settings.json` |
| [`discovery::project_settings_path`](../../crates/cyrup-ext-subagents/src/discovery/mod.rs) (`:364`) | `<root>/.cyrup/**agents**/settings.json` |

`agent/` vs `agents/`. Four distinct files;
[`registration/profiles.rs:230-232`](../../crates/cyrup-ext-subagents/src/registration/profiles.rs)
already records the distinction in a comment. So there is **no** in-tree pair of `FileLock` call
sites that reaches one file by two spellings today.

**(b) The real collision is one env var away, and it is a cross-crate one.** The two chains use
different home resolvers —
[`ConfigDirs::resolve`](../../crates/cyrup-config/src/env.rs) uses `directories::BaseDirs`,
[`dirs_home()`](../../crates/cyrup-ext-subagents/src/extension/executor/paths.rs) (`:108`) uses
`$CYRUP_HOME` → `$HOME` → `temp_dir()`. Set `CYRUP_AGENT_DIR=$HOME/.cyrup/agents` and
`cyrup-config`'s `settings_path()` and `cyrup-ext-subagents`' `user_settings_path` name **one file
by two independently-built spellings**, from two crates, both of which reach
[`FileLock::acquire`](../../crates/cyrup-config/src/lock.rs) (via
[`settings/store.rs:69`](../../crates/cyrup-config/src/settings/store.rs) and
[`discovery/settings_write.rs:81-89`](../../crates/cyrup-ext-subagents/src/discovery/settings_write.rs)).
They can also differ whenever `$CYRUP_HOME` is set at all.

**(c) A relative key is worse than a duplicate key, and it is reachable through documented config.**
[`EnvVars::from_lookup`'s `path` helper (`env.rs:79`)](../../crates/cyrup-config/src/env.rs) applies
only `normalize_path_buf` — tilde and `file://` expansion, explicitly *not* `resolve`
([`paths.rs:24-26`](../../crates/cyrup-config/src/paths.rs): "this is NOT `resolve`, it does not
make a relative path absolute"). So `--agent-dir ./cfg` or `CYRUP_AGENT_DIR=.cyrup-test` yields a
**relative** `agent_dir`, hence relative `settings_path()` / `auth_path()` / `trust_path()` /
`models_path()`. A relative key does not merely split one file across two entries; it makes one
entry ambiguous — the same `PathBuf` names whatever the cwd points at when the lock is taken, while
layer 2's `open(2)` resolves it against the cwd at open time. The two layers stop agreeing about
what they are protecting. Nothing in-tree calls `std::env::set_current_dir`, so this is latent
rather than live, but it is the failure mode with teeth.

**(d) The suggested fix has a hole exactly where this crate needs it most.** The review proposes
canonicalizing the *parent* and rejoining. The parent frequently does not exist:
[`open_and_lock` (`lock.rs:74-89`)](../../crates/cyrup-config/src/lock.rs) is what runs `ensure_dir`
and `create(true)` — the lock is what *creates* the agent dir and the sidecar on a fresh install. A
`realpath` therefore returns `ENOENT` on precisely the first-run acquires and falls back to the
unresolved spelling, leaving the gap it was added to close, for the case where two tasks racing to
create the same file is most likely.

## 3. The decisive evidence: upstream resolves these two domains differently, on purpose

A vendored pi is in [`tmp/pi`](../../tmp/pi). Every synchronous lock pi takes on **these three
files** passes `realpath: false` explicitly:

| site | call |
| --- | --- |
| [`settings-manager.ts:206`](../../tmp/pi/packages/coding-agent/src/core/settings-manager.ts) | `lockfile.lockSync(path, { realpath: false })` |
| [`trust-manager.ts:145`](../../tmp/pi/packages/coding-agent/src/core/trust-manager.ts) | `lockfile.lockSync(trustDir, { realpath: false, lockfilePath: \`${path}.lock\` })` |
| [`auth-storage.ts:56`](../../tmp/pi/packages/coding-agent/src/core/auth-storage.ts) | `lockfile.lockSync(path, { realpath: false })` |

`proper-lockfile@4.1.2` defaults `realpath` to **true**; these three opt out by hand. Instead,
upstream makes the key unambiguous at *construction*, lexically —
[`settings-manager.ts:133-137`](../../tmp/pi/packages/coding-agent/src/core/settings-manager.ts):

```ts
constructor(cwd: string, agentDir: string) {
    const resolvedCwd = resolvePath(cwd);
    const resolvedAgentDir = resolvePath(agentDir);
    this.globalSettingsPath = join(resolvedAgentDir, "settings.json");
    this.projectSettingsPath = join(resolvedCwd, CONFIG_DIR_NAME, "settings.json");
}
```

`resolvePath` is node's lexical `path.resolve`
([`utils/paths.ts:81-85`](../../tmp/pi/packages/coding-agent/src/utils/paths.ts)) — never
`realpathSync`, which pi reserves for the two places that genuinely compare realpaths
(`trust-manager.ts` `normalizeCwd`, `resource-loader.ts` `findShadowedContextFile`). Meanwhile
`cyrup-tools`' upstream, `getMutationQueueKey`
(`file-mutation-queue.ts:16-26`, quoted in full at
[`cyrup-tools/src/lock.rs:96-103`](../../crates/cyrup-tools/src/lock.rs)), *does* `await realpath`.

**The two cyrup domains resolve differently because the two upstreams do.** `keyed_lock.rs:8-10` is
what is wrong: it states one flat notion of "resolved" for a mechanism that deliberately leaves the
choice to the domain. `cyrup-config` is not skipping the obligation — it is missing the *lexical*
half of it, which is the half its upstream performs.

(The one asymmetry worth knowing: `auth-storage.ts:111`'s async `lockfile.lock(this.authPath, …)`
does **not** pass `realpath: false`, so that single site inherits the `realpath: true` default. It is
one of four, and it is the outlier; it does not move the decision.)

## 4. Cost, weighed honestly

| | `tokio::fs::canonicalize` (the `cyrup-tools` shape) | lexical resolve |
| --- | --- | --- |
| per acquire | a `spawn_blocking` hop + `realpath(2)` | a `components()` walk; **no syscall** when the path is already absolute, one `getcwd(3)` when it is not |
| first-run (`ENOENT`) | falls back to the raw spelling — gap stays open | works identically; nothing is stat'ed |
| blocking pool | one more `spawn_blocking` on a path two sibling findings are trying to make *less* pool-hungry | none |
| Windows | emits `\\?\`-verbatim keys; [`env.rs:214-241`](../../crates/cyrup-config/src/env.rs) removed a `canonicalize` for precisely that reason | unaffected |
| symlink aliases | closed | still two keys — see below |
| upstream | contradicts `realpath: false` at 3 of 4 sites | matches `resolvePath(agentDir)` exactly |

The symlink residue is the only thing lexical resolution gives up, and it costs nothing but one
extra `flock` round trip, because **layer 2 is `flock` on an inode**. Both aliases reach the same
inode, so mutual exclusion holds either way. Upstream cannot say that — `proper-lockfile`'s lock is
a `mkdir` of a *path*, so two symlink spellings genuinely get two independent locks there. cyrup is
strictly stronger than pi even with the residue, which is exactly why the residue is acceptable and
exactly what the doc must say instead of claiming it does not exist.

**Required path: resolve lexically in `cyrup-config`, and correct the contract to say that
"resolved" is the domain's definition.** Not a choice between "fix the code" and "fix the claim" —
both halves are one change, because the claim is wrong in a way the code fix does not by itself
repair.

---

## 5. Implementation

Four edits, three files. No public API change —
[`LOW-public-api-changes-beyond-the-async-keyword`](./LOW-public-api-changes-beyond-the-async-keyword.md)
tracks public-surface drift on this branch, so keep the surface fixed: `lock_key` is private and
`lexically_normalize` goes no further than `pub(crate)`.

### 5.1 `crates/cyrup-config/src/paths.rs` — expose the existing normalizer

[`lexically_normalize` (`:156-179`)](../../crates/cyrup-config/src/paths.rs) is already exactly node's
post-join `.`/`..` collapse, already `Path`/`OsStr`-safe (no `String` round trip, so a non-UTF-8 path
cannot be mangled into a colliding key), and already the thing
`resolve_path_from_base_with_home` finishes with. Reuse it rather than writing a second normalizer.

```rust
/// The `.` / `..` collapse node's `path.resolve` performs after joining — purely lexical.
///
/// `..` at the root is dropped rather than escaping it (`path.resolve("/a/../..") === "/"`); on a
/// relative remainder it is kept, because there is nothing to cancel it against.
///
/// `pub(crate)` for [`crate::lock::FileLock`]'s layer-1 key, which needs this collapse WITHOUT the
/// tilde / `file://` tier above it and without a `String` round trip: a lock key must survive a
/// non-UTF-8 path byte-for-byte, or two distinct files can hash to one entry.
pub(crate) fn lexically_normalize(path: &std::path::Path) -> PathBuf {
```

Only the visibility and the added paragraph change; the body stays as-is.

### 5.2 `crates/cyrup-config/src/lock.rs` — the key helper

Add immediately after [`lock_path_for` (`:108-118`)](../../crates/cyrup-config/src/lock.rs):

```rust
/// Layer 1's key: [`lock_path_for`]'s sidecar path made **lexically absolute**, so every spelling of
/// one config file that a caller can hand [`FileLock::acquire`] hashes to one entry in
/// [`CONFIG_LOCKS`].
///
/// Node `path.resolve` semantics, NOT `realpath`, and that is upstream's own choice rather than a
/// shortcut. Every sync lock pi takes on these files opts out of realpath by hand —
/// `settings-manager.ts:206`, `trust-manager.ts:145`, `auth-storage.ts:56` all pass
/// `{ realpath: false }` against `proper-lockfile`'s `realpath: true` default — and pi makes the key
/// unambiguous at CONSTRUCTION instead, with `resolvePath(cwd)` / `resolvePath(agentDir)`
/// (settings-manager.ts:133-137), which is node's lexical `path.resolve` and touches no filesystem.
/// `cyrup-tools`' `FileMutationLocks::key` realpaths because ITS upstream does
/// (`getMutationQueueKey`, file-mutation-queue.ts:16-26). The two domains over
/// [`cyrup_core::keyed_lock`] resolve differently because the two upstreams do.
///
/// A realpath would also be actively worse here: [`open_and_lock`] is what runs [`ensure_dir`] and
/// `create(true)`, so this lock is routinely taken on a file — and inside a directory — that does
/// not exist yet. `realpath` returns `ENOENT` on exactly those first-run acquires and would fall
/// back to the unresolved spelling, leaving the gap open for the case where two tasks racing to
/// create one config file is most likely. It would additionally put a Windows `\\?\`-verbatim form
/// into the key, which is the divergence `ConfigDirs::resolve` removed a `canonicalize` to avoid
/// (env.rs, the SESS-036 note).
///
/// Residue, stated rather than papered over: two spellings that differ only through a **symlink**
/// still produce two keys, hence two layer-2 waiters from this process. That costs one extra `flock`
/// round trip and nothing else, because layer 2 is `flock` on an **inode** — both aliases reach the
/// same one, so mutual exclusion holds regardless of the name it was reached by. Upstream's
/// `mkdir`-based lock is path-keyed and does NOT hold under that alias, so this is stronger than pi
/// even with the residue.
fn lock_key(lock_path: &Path) -> PathBuf {
    if lock_path.is_absolute() {
        return crate::paths::lexically_normalize(lock_path);
    }
    // `getcwd(3)` — a plain syscall on the rare relative branch, not a blocking-pool hop. A
    // relative key would otherwise name whatever the cwd points at when the lock is taken, while
    // `open(2)` in `open_and_lock` resolves it against the cwd at open time: the two layers would
    // stop agreeing about which file they protect. `--agent-dir ./cfg` and `CYRUP_AGENT_DIR=./cfg`
    // reach here, because `EnvVars`' `normalize_path_buf` expands `~` and `file://` and stops
    // (paths.ts:57-78 — "NOT `resolve`").
    match std::env::current_dir() {
        Ok(cwd) => crate::paths::lexically_normalize(&cwd.join(lock_path)),
        // No reachable cwd: the `open(2)` below is about to fail on the same relative path, so keep
        // the raw spelling and let layer 2 produce the error that names it.
        Err(_) => lock_path.to_path_buf(),
    }
}
```

Then change **one argument** in [`acquire` (`:57-63`)](../../crates/cyrup-config/src/lock.rs). The
`lock_path.clone()` goes away: `lock_key` borrows, and `lock_path` still moves into the blocking
closure unchanged.

```rust
        // Layer 1 keys on the RESOLVED sidecar path; layer 2 opens the caller's own spelling. They
        // must stay separate: `flock` is inode-based, so the raw spelling reaches the same lock,
        // while `ConfigError::Io`/`ConfigError::Lock` keep naming the path the operator typed.
        let in_process = CONFIG_LOCK_HANDLE
            .guard(lock_key(&lock_path), token)      // was: lock_path.clone()
            .await
            .map_err(|_| ConfigError::Cancelled)?;
```

That single substitution is the whole behavioural change, and it is why it composes with the
`acquire` restructure the MEDIUM sibling specifies (§6): whatever else moves around it, the
`guard(...)` call keeps taking a key and `open_and_lock` keeps taking `lock_path`.

Do **not** substitute the resolved key for `lock_path` inside `open_and_lock` — see §6.

### 5.3 `crates/cyrup-config/src/lock.rs` — the `FileLock` type doc (`:35-41`)

Both sibling findings rewrite this same paragraph, and they rewrite it in **opposite directions**
(§6). So this task deliberately owns none of the blocking-vs-polling argument. It owns exactly two
things, both phrased to be independent of how layer 2 waits:

**(i) One phrase in the first sentence.** Wherever that paragraph says the queue is on a
"per-**path**" mutex admitting one task "per **path** per process", it becomes per-**key**, with the
key named:

```rust
/// Two layers. In-process contention — several tasks in one cyrup process reaching the same config
/// file — queues on a per-key async mutex, the key being the sidecar path made lexically absolute by
/// [`lock_key`]: fair, cancel-aware, no syscall and no polling. Every spelling of one file that does
/// not differ through a symlink hashes to one entry, so layer 1 admits at most one task per file per
/// process, …
```

…and the rest of that sentence continues into whatever the surviving text says about layer 2.

**(ii) One appended paragraph**, self-contained, added after the paragraph above:

```rust
/// The one alias layer 1 does not merge is a **symlink**: two such spellings are two keys, hence two
/// layer-2 waiters from this process on one inode. It costs an extra round trip, never correctness —
/// layer 2 locks the **inode**, so it excludes both aliases whichever name reached it. Upstream's
/// `mkdir` lock is path-keyed and does not, which is why this residue is affordable here and would
/// not have been in pi. Resolving it away would mean a `realpath`, and [`lock_key`] documents why
/// this domain must not take one.
```

Neither snippet mentions `flock`-vs-poll, `spawn_blocking`, cancellation or the blocking pool, so it
applies verbatim on top of either sibling's rewrite.

[`auth.rs:269-272`](../../crates/cyrup-config/src/auth.rs)'s "keys per FILE" claim becomes *more*
true after this and needs no edit.

### 5.4 `crates/cyrup-core/src/keyed_lock.rs` — the contract (`:8-10`)

Replace the flat obligation with one that states the real requirement and hands the choice back to
the domain:

```rust
//! Keying is the caller's job too, and it is the caller's whole job: this module hashes the key it
//! is handed and nothing else, so two keys naming one entity are two locks. A path-keyed domain
//! must therefore reduce every spelling it can be handed to a single form BEFORE calling
//! [`KeyedLocks::guard`] — and it must choose WHICH form, because this module never touches the
//! filesystem (see the crate docs: no I/O) and so cannot choose for it. The two in-tree domains
//! choose differently, on purpose: `cyrup-tools`' `FileMutationLocks` realpaths, because its
//! upstream `getMutationQueueKey` does; `cyrup-config`'s `FileLock` resolves lexically, because its
//! upstream calls `proper-lockfile` with `realpath: false` and makes the path absolute at
//! construction instead. "Resolved" is the domain's definition, not this module's.
```

---

## 6. Interaction with the two sibling findings (do not edit their files)

Both siblings are now `stage: aug, status: done`, both rewrite
[`lock.rs`](../../crates/cyrup-config/src/lock.rs) in the same region, and — read them before
executing — **they prescribe opposite fixes and each declares the other subsumed**:

- [`HIGH-dropped-acquire-future-detaches-blocking-flock-task`](./HIGH-dropped-acquire-future-detaches-blocking-flock-task.md)
  replaces the blocking `flock` with `fs4`'s non-blocking `FileExt::try_lock` retried from async
  land (`tokio::pin!`ed `cancelled()` + a `biased` `select!` + capped backoff), and says "do not also
  apply MEDIUM's suggested fix".
- [`MEDIUM-cancel-token-does-not-reach-the-cross-process-flock-wait`](./MEDIUM-cancel-token-does-not-reach-the-cross-process-flock-wait.md)
  keeps the blocking `flock`, moves the layer-1 `KeyedGuard` **into** the blocking closure, races
  the token against the join handle, and says "if the HIGH task is instead executed with its
  primary (polling) suggestion, this spec is void".

**That contradiction is theirs to settle and is not a blocker for this task.** This change is
deliberately shaped so it is a no-op on that argument, and lands identically either way:

- **The code edit is one argument.** Both siblings keep the same two statements in the same order —
  `CONFIG_LOCK_HANDLE.guard(lock_path.clone(), token).await`, then a `spawn_blocking` that takes
  `&lock_path`. §5.2 changes only the first argument of `guard`, above everything either sibling
  restructures. The HIGH's own Interactions section already records this ("changes how `lock_path`
  is *derived* before `CONFIG_LOCK_HANDLE.guard` … does not interact with the retry loop").
  Under the HIGH's shape `lock_path` moves into the *first* `spawn_blocking` and later retries carry
  only `file`; `lock_key(&lock_path)` borrows before that move, so it composes unchanged. Under the
  MEDIUM's shape `lock_path` moves into the one closure that also owns the guard; likewise unchanged.
- **The doc edit is two conflict-free fragments** (§5.3): a per-path → per-key phrase swap in the
  first sentence, and one appended paragraph that says nothing about how layer 2 waits. The MEDIUM
  explicitly asks for this ("the two edits must compose rather than overwrite each other").
- **What this task does NOT fix.** A dropped or cancelled acquire is a *lifetime* defect; this is a
  *key-space* defect. Neither implies the other: after this change a dropped future still strands a
  layer-2 attempt, and after either sibling's change two symlink spellings would still be two
  waiters. What this removes is one of the two independent ways the "exactly one waiter" sentence can
  be false, so whichever sibling rewrites that sentence only has to account for the lifetime half.
- **Hard constraint for both siblings' code: `open_and_lock` / `open_and_try_lock` keeps receiving
  `lock_path`, the caller's raw spelling — never `lock_key`'s output.** `flock` is inode-based, so
  the raw spelling reaches the same lock, and keeping it is what stops a resolved (or, on Windows,
  `\\?\`-verbatim) form from leaking into `ConfigError::Io { path }` / `ConfigError::Lock { path }`
  and being shown to an operator who never typed it. The resolved value exists only as a hash key and
  must never be rendered.

## 7. Explicitly not in scope

- **Do not canonicalize.** §3 and §4 are the reasons; re-adding a `realpath` to this crate also
  re-opens the `\\?\` divergence `ConfigDirs::resolve` documents at length.
- **Do not change `ConfigDirs::resolve`** to `resolvePath(agent_dir)` for pi parity. `agent_dir`
  flows into session directory names, the `"cwd"` field in every session header, and rendered
  `<project_instructions path="…">` — absolutizing it changes user-visible output far beyond the
  lock, and would not cover `cyrup-ext-subagents`' externally-built paths anyway. The lock is the
  choke point every caller already passes through; fix it there.
- **Do not unify the two domains onto one keying function.** `cyrup-core` cannot do I/O, and the two
  domains are correct to differ. The contract sentence is what must change.
- **Do not touch `lock_path_for`.** It stays a pure lexical `<name>.lock` transform; resolution is a
  separate, separately-documented step.

## 8. Definition of done

1. `crates/cyrup-config/src/paths.rs`: `lexically_normalize` is `pub(crate)` and carries the added
   paragraph. Body unchanged. No new item in [`lib.rs`'s `pub use paths::{…}`](../../crates/cyrup-config/src/lib.rs).
2. `crates/cyrup-config/src/lock.rs`: `lock_key` exists with the documentation in §5.2; `acquire`
   passes `lock_key(&lock_path)` to `CONFIG_LOCK_HANDLE.guard`; the stray `lock_path.clone()` is
   gone; and the untouched `lock_path` is still what reaches the open/lock helper (`open_and_lock`,
   or `open_and_try_lock` if the HIGH sibling landed first) — no resolved form is reachable from any
   rendered `ConfigError`.
3. `crates/cyrup-config/src/lock.rs`'s type doc states the invariant per **key** (naming
   [`lock_key`]) rather than per path, and carries §5.3(ii)'s symlink-residue paragraph. Whatever a
   sibling finding wrote about *how* layer 2 waits is still there — this task overwrote none of it.
4. `crates/cyrup-core/src/keyed_lock.rs:8-10` is replaced by §5.4 — no sentence anywhere still
   implies both domains resolve the same way.
5. Grepping the tree for `one task per path` / `keys per FILE` / `exactly one waiter` turns up no
   remaining claim that the new key space does not support.
6. `cargo clippy -p cyrup-config -p cyrup-core` is clean (the workspace denies `unwrap_used`,
   `expect_used`, `panic`, `indexing_slicing` — `lock_key` uses none of them), and
   `cargo fmt -p cyrup-config -p cyrup-core` leaves the two touched files unchanged.
