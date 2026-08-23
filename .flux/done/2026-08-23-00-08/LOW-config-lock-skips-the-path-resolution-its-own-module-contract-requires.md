---
title: Config Lock Skips The Path Resolution Its Own Module Contract Requires
priority: LOW
stage: qa
status: completed
updated: 2026-08-23 07:54
---

# Make `cyrup-config`'s lock key canonical for its own key space, and correct the contract that says both domains resolve the same way

OBJECTIVE: layer 1 of [`cyrup-config`'s `FileLock`](../../crates/cyrup-config/src/lock.rs) must key
on a **lexically absolute** sidecar path, and the two docs that assert "one task per path per
process" as a flat fact must state what it actually guarantees — including the one alias it does not
cover, and why that residue is harmless *here* and would not be upstream.

Five edits, three files: one visibility change, one new private helper, one argument, two doc
blocks. Most of what follows is the research that fixes the required path, because the review's
suggested fix ("resolve, matching `cyrup-tools`") is the **wrong** resolution for this domain and
the evidence for that is not in either lock file.

> **Citation freshness.** Every `file:line` below was re-verified against the working tree on
> 2026-08-23 05:51. `crates/cyrup-config/src/lock.rs` was rewritten in the interim by the HIGH
> sibling (see §6) — `open_and_lock` no longer exists, the `FileLock` type doc moved from `:35-41`
> to `:50-73`, and `acquire` moved from `:57-63` to `:103-150`. Prior revisions of this spec cited
> the pre-rewrite lines and were wrong. Where a pointer can be a **name**, it is a name.

---

## 1. What the review got right

The mismatch is real. The module doc of
[`cyrup-core/src/keyed_lock.rs`](../../crates/cyrup-core/src/keyed_lock.rs) (`:8-10`, verified)
states a flat obligation:

> Keying is the caller's job too. This module never touches the filesystem (see the crate docs:
> no I/O), so a domain that keys on resolved paths resolves them itself before calling
> [`KeyedLocks::guard`].

`cyrup-tools`' `FileMutationLocks::key`
([`crates/cyrup-tools/src/lock.rs`](../../crates/cyrup-tools/src/lock.rs), fn at `:118-126`)
honours it with `tokio::fs::canonicalize` + an ENOENT/ENOTDIR fallback.

`cyrup-config`'s `FileLock::acquire`
([`crates/cyrup-config/src/lock.rs`](../../crates/cyrup-config/src/lock.rs), fn at `:103-150`) does
not — it hands `lock_path.clone()` straight to `CONFIG_LOCK_HANDLE.guard` (`lock.rs:107`), and
`lock_path_for` (`lock.rs:211-221`) is a pure string transform (`parent().join(file_name +
".lock")`) with no resolution of any kind.

And the invariant that rests on it is asserted in **two** live places (the third listed in earlier
revisions of this spec was in a sibling task file, not in source):

- the `FileLock` type doc, `lock.rs:53-56` — "admits at most one task per path per process, so this
  process presents at most one waiter to the cross-process lock";
- `auth.rs:307-308` (**not** `:269-272` — that citation was stale) — "`FileLock`'s in-process layer
  keys per FILE", used to argue the file lock subsumes the per-provider mutex.

A whole-tree grep for the phrasings returns exactly these two hits and nothing else:

```
$ rg -n 'one task per path|per path per process|keys per FILE|exactly one waiter' crates/
crates/cyrup-config/src/lock.rs:55:/// admits at most one task per path per process, so this process presents at most one waiter to the
crates/cyrup-config/src/auth.rs:308:        // `FileLock`'s in-process layer keys per FILE, and every provider shares one auth file. So
```

## 2. What the review got wrong — corrections that change the fix

**(a) The two `settings.json` files are not the same file.** The review says
`FileSettingsStore.{global_path, project_path}` (fields at
[`settings/store.rs:29-30`](../../crates/cyrup-config/src/settings/store.rs)) and
`cyrup-ext-subagents`' `user_settings_path` are "two separately constructed spellings that, on a
default layout, name the same `settings.json`". They are not, on any layout that isn't hand-made:

| producer | default path |
| --- | --- |
| `ConfigDirs::settings_path()` ([`env.rs:311`](../../crates/cyrup-config/src/env.rs)) | `<agent_dir>/settings.json` = `~/.cyrup/`**`agent`**`/settings.json` (`agent_dir` default at `env.rs:194`) |
| `ConfigDirs::project_settings_path()` ([`env.rs:323`](../../crates/cyrup-config/src/env.rs), via `project_config_dir()` at `:320`) | `<cwd>/.cyrup/settings.json` |
| `SubagentExecutor::discovery_config` ([`resolve.rs:112`](../../crates/cyrup-ext-subagents/src/extension/executor/resolve.rs)) | `dirs_home()/.cyrup/`**`agents`**`/settings.json` |
| `discovery::project_settings_path` ([`discovery/mod.rs:321`](../../crates/cyrup-ext-subagents/src/discovery/mod.rs) — **not** `:364`) | `<root>/.cyrup/`**`agents`**`/settings.json` |

`agent/` vs `agents/`. Four distinct files;
[`registration/profiles.rs:229-233`](../../crates/cyrup-ext-subagents/src/registration/profiles.rs)
(**not** `:230-232`) already records the distinction in a comment. So there is **no** in-tree pair
of `FileLock` call sites that reaches one file by two spellings today.

**(b) The real collision is one env var away, and it is a cross-crate one.** The two chains use
different home resolvers — `ConfigDirs::resolve`
([`env.rs:177-178`](../../crates/cyrup-config/src/env.rs)) uses `directories::BaseDirs`;
`dirs_home()` ([`extension/executor/paths.rs:108-113`](../../crates/cyrup-ext-subagents/src/extension/executor/paths.rs))
uses `$CYRUP_HOME` → `$HOME` → `temp_dir()`. Set `CYRUP_AGENT_DIR=$HOME/.cyrup/agents` and
`cyrup-config`'s `settings_path()` and `cyrup-ext-subagents`' `user_settings_path` name **one file
by two independently-built spellings**, from two crates, both of which reach `FileLock::acquire`
(via [`settings/store.rs:69`](../../crates/cyrup-config/src/settings/store.rs) and
`discovery/settings_write.rs`'s `lock_settings_file`,
[`:81-90`](../../crates/cyrup-ext-subagents/src/discovery/settings_write.rs)). They can also differ
whenever `$CYRUP_HOME` is set at all.

**(c) A relative key is worse than a duplicate key, and it is reachable through documented config.**
`EnvVars::from_lookup`'s `path` helper ([`env.rs:79`](../../crates/cyrup-config/src/env.rs)) applies
only `normalize_path_buf` — tilde and `file://` expansion, explicitly *not* `resolve`
([`paths.rs:24-26`](../../crates/cyrup-config/src/paths.rs): "this is NOT `resolve`, it does not
make a relative path absolute"). So `--agent-dir ./cfg` or `CYRUP_AGENT_DIR=.cyrup-test` yields a
**relative** `agent_dir`, hence relative `settings_path()` / `auth_path()` / `trust_path()` /
`models_path()` (`env.rs:311`, `:317`, `:314`, `:330`). A relative key does not merely split one
file across two entries; it makes one entry ambiguous — the same `PathBuf` names whatever the cwd
points at when the lock is taken, while layer 2's `open(2)` in `open_and_try_lock`
(`lock.rs:156-168`) resolves it against the cwd at open time. The two layers stop agreeing about
what they are protecting. Nothing in-tree calls `std::env::set_current_dir`, so this is latent
rather than live, but it is the failure mode with teeth.

**(d) The suggested fix has a hole exactly where this crate needs it most.** The review proposes
canonicalizing the *parent* and rejoining. The parent frequently does not exist: `open_and_try_lock`
(`lock.rs:156-168`) is what runs `ensure_dir(parent)` and `OpenOptions::create(true)` — the lock is
what *creates* the agent dir and the sidecar on a fresh install. A `realpath` therefore returns
`ENOENT` on precisely the first-run acquires and falls back to the unresolved spelling, leaving the
gap it was added to close, for the case where two tasks racing to create the same file is most
likely.

## 3. The decisive evidence: upstream resolves these two domains differently, on purpose

A vendored pi is in [`tmp/pi`](../../tmp/pi). Every synchronous lock pi takes on **these three
files** passes `realpath: false` explicitly (all three lines re-verified):

| site | call |
| --- | --- |
| `packages/coding-agent/src/core/settings-manager.ts:206` | `return lockfile.lockSync(path, { realpath: false });` |
| `packages/coding-agent/src/core/trust-manager.ts:145` | `return lockfile.lockSync(trustDir, { realpath: false, lockfilePath: ... });` |
| `packages/coding-agent/src/core/auth-storage.ts:56` | `return lockfile.lockSync(path, { realpath: false });` |

`proper-lockfile@4.1.2` defaults `realpath` to **true**; these three opt out by hand. Instead,
upstream makes the key unambiguous at *construction*, lexically — `SettingsManager`'s constructor,
`settings-manager.ts:192-196` (**not** `:133-137` — that citation was stale):

```ts
constructor(cwd: string, agentDir: string) {
    const resolvedCwd = resolvePath(cwd);
    const resolvedAgentDir = resolvePath(agentDir);
    this.globalSettingsPath = join(resolvedAgentDir, "settings.json");
    this.projectSettingsPath = join(resolvedCwd, CONFIG_DIR_NAME, "settings.json");
}
```

`resolvePath` is node's lexical `path.resolve` (`packages/coding-agent/src/utils/paths.ts:81-85`,
verified) — never `realpathSync`, which pi reserves for the two places that genuinely compare
realpaths (`trust-manager.ts` `normalizeCwd`, `resource-loader.ts` `findShadowedContextFile`; both
already catalogued in this tree at `env.rs:218-224`). Meanwhile `cyrup-tools`' upstream,
`getMutationQueueKey` (`file-mutation-queue.ts:16-26`, quoted in full in `FileMutationLocks::key`'s
doc at [`cyrup-tools/src/lock.rs:95-103`](../../crates/cyrup-tools/src/lock.rs)), *does*
`await realpath`.

**The two cyrup domains resolve differently because the two upstreams do.** `keyed_lock.rs:8-10` is
what is wrong: it states one flat notion of "resolved" for a mechanism that deliberately leaves the
choice to the domain. `cyrup-config` is not skipping the obligation — it is missing the *lexical*
half of it, which is the half its upstream performs.

(The one asymmetry worth knowing: `auth-storage.ts:111`'s async `lockfile.lock(this.authPath, …)`
does **not** pass `realpath: false`, so that single site inherits the `realpath: true` default. It is
one of four, and it is the outlier; it does not move the decision.)

There is also in-tree precedent for the exact shape this task prescribes:
[`cyrup-session/src/git_paths.rs:99-106`](../../crates/cyrup-session/src/git_paths.rs)'s
`resolve_path` is already "absolutize against the process cwd, then lexically collapse", documented
as "Deliberately NOT `std::fs::canonicalize`". `lock_key` is that same shape. (Note it carries its
own private `lexically_normalize` at `git_paths.rs:110` — a *different* function in a different
crate. Do not touch it, and do not confuse it with `cyrup-config`'s.)

## 4. Cost, weighed honestly

| | `tokio::fs::canonicalize` (the `cyrup-tools` shape) | lexical resolve |
| --- | --- | --- |
| per acquire | a `spawn_blocking` hop + `realpath(2)` | a `components()` walk; **no syscall** when the path is already absolute, one `getcwd(3)` when it is not |
| first-run (`ENOENT`) | falls back to the raw spelling — gap stays open | works identically; nothing is stat'ed |
| blocking pool | one more `spawn_blocking` on a path the HIGH sibling just made *less* pool-hungry | none |
| Windows | emits `\\?\`-verbatim keys; [`env.rs:214-236`](../../crates/cyrup-config/src/env.rs) (**not** `:214-241`) removed a `canonicalize` for precisely that reason | unaffected |
| symlink aliases | closed | still two keys — see below |
| upstream | contradicts `realpath: false` at 3 of 4 sites | matches `resolvePath(agentDir)` exactly |

The symlink residue is the only thing lexical resolution gives up, and it costs nothing but one
extra retry round trip, because **layer 2 is `flock` on an inode**. Both aliases reach the same
inode, so mutual exclusion holds either way. Upstream cannot say that — `proper-lockfile`'s lock is
a `mkdir` of a *path*, so two symlink spellings genuinely get two independent locks there. cyrup is
strictly stronger than pi even with the residue, which is exactly why the residue is acceptable and
exactly what the doc must say instead of claiming it does not exist.

**Required path: resolve lexically in `cyrup-config`, and correct the contract to say that
"resolved" is the domain's definition.** Not a choice between "fix the code" and "fix the claim" —
both halves are one change, because the claim is wrong in a way the code fix does not by itself
repair.

---

## 5. Implementation — exact, byte-for-byte edits

Five edits, three files. No public API change —
[`LOW-public-api-changes-beyond-the-async-keyword`](./LOW-public-api-changes-beyond-the-async-keyword.md)
tracks public-surface drift on this branch, so keep the surface fixed: `lock_key` is private and
`lexically_normalize` goes no further than `pub(crate)`.

Every FIND block below was verified to occur **exactly once** in its file at 2026-08-23 05:51.
Before each edit, re-assert that — the count must be `1`:

```
$ rg -F --count-matches '<first line of the FIND block>' <file>
```

Apply the edits in order. Do not reflow, re-wrap, or reformat any line that is not shown.

### 5.1 `crates/cyrup-config/src/paths.rs` — expose the existing normalizer

`lexically_normalize` (doc `:156-159`, fn `:160-179`) is already exactly node's post-join `.`/`..`
collapse, already `Path`/`OsStr`-safe (no `String` round trip, so a non-UTF-8 path cannot be mangled
into a colliding key), and already the thing `resolve_path_from_base_with_home` finishes with
(`paths.rs:153`). Reuse it rather than writing a second normalizer.

FIND (count 1 in `crates/cyrup-config/src/paths.rs`):

```rust
/// The `.` / `..` collapse node's `path.resolve` performs after joining — purely lexical.
///
/// `..` at the root is dropped rather than escaping it (`path.resolve("/a/../..") === "/"`); on a
/// relative remainder it is kept, because there is nothing to cancel it against.
fn lexically_normalize(path: &std::path::Path) -> PathBuf {
```

REPLACE WITH:

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

Only the visibility and the added paragraph change; the body stays as-is. Nothing is added to
[`lib.rs`'s `pub use paths::{…}`](../../crates/cyrup-config/src/lib.rs) (`lib.rs:67-69`), so the
crate's public surface is untouched.

### 5.2 `crates/cyrup-config/src/lock.rs` — add the key helper

Insert `lock_key` immediately after `lock_path_for` (`lock.rs:211-221`). `lock_path_for` currently
carries no doc comment; leave it that way — see §7.

FIND (count 1 in `crates/cyrup-config/src/lock.rs`):

```rust
fn lock_path_for(target: &Path) -> PathBuf {
    let mut name = target
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".lock");
    match target.parent() {
        Some(p) => p.join(name),
        None => PathBuf::from(name),
    }
}
```

REPLACE WITH (the same block, then a blank line, then the new fn):

```rust
fn lock_path_for(target: &Path) -> PathBuf {
    let mut name = target
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".lock");
    match target.parent() {
        Some(p) => p.join(name),
        None => PathBuf::from(name),
    }
}

/// Layer 1's key: [`lock_path_for`]'s sidecar path made **lexically absolute**, so every spelling of
/// one config file that a caller can hand [`FileLock::acquire`] hashes to one entry in
/// [`CONFIG_LOCKS`].
///
/// Node `path.resolve` semantics, NOT `realpath`, and that is upstream's own choice rather than a
/// shortcut. Every sync lock pi takes on these files opts out of realpath by hand —
/// `settings-manager.ts:206`, `trust-manager.ts:145`, `auth-storage.ts:56` all pass
/// `{ realpath: false }` against `proper-lockfile`'s `realpath: true` default — and pi makes the key
/// unambiguous at CONSTRUCTION instead, with `resolvePath(cwd)` / `resolvePath(agentDir)`
/// (settings-manager.ts:192-196), which is node's lexical `path.resolve` (utils/paths.ts:81-85) and
/// touches no filesystem. `cyrup-tools`' `FileMutationLocks::key` realpaths because ITS upstream
/// does (`getMutationQueueKey`, file-mutation-queue.ts:16-26). The two domains over
/// [`cyrup_core::keyed_lock`] resolve differently because the two upstreams do.
///
/// A realpath would also be actively worse here: [`open_and_try_lock`] is what runs [`ensure_dir`]
/// and `create(true)`, so this lock is routinely taken on a file — and inside a directory — that
/// does not exist yet. `realpath` returns `ENOENT` on exactly those first-run acquires and would
/// fall back to the unresolved spelling, leaving the gap open for the case where two tasks racing
/// to create one config file is most likely. It would additionally put a Windows `\\?\`-verbatim
/// form into the key, which is the divergence `ConfigDirs::resolve` removed a `canonicalize` to
/// avoid (env.rs, the SESS-036 note).
///
/// Residue, stated rather than papered over: two spellings that differ only through a **symlink**
/// still produce two keys, hence two layer-2 waiters from this process. That costs an extra retry
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
    // `open(2)` in `open_and_try_lock` resolves it against the cwd at open time: the two layers
    // would stop agreeing about which file they protect. `--agent-dir ./cfg` and
    // `CYRUP_AGENT_DIR=./cfg` reach here, because `EnvVars`' `normalize_path_buf` expands `~` and
    // `file://` and stops — see [`crate::paths::normalize_path`], "this is NOT `resolve`".
    match std::env::current_dir() {
        Ok(cwd) => crate::paths::lexically_normalize(&cwd.join(lock_path)),
        // No reachable cwd: the `open(2)` in `open_and_try_lock` is about to fail on the same
        // relative path, so keep the raw spelling and let layer 2 produce the error that names it.
        Err(_) => lock_path.to_path_buf(),
    }
}
```

`Path::join` with an absolute argument already discards the base, so the `is_absolute` branch is a
pure `getcwd(3)` elision on the common path, not a correctness fork. Write it anyway — that elision
is the whole per-acquire cost argument in §4.

### 5.3 `crates/cyrup-config/src/lock.rs` — use the key in `acquire`

One argument changes. The `lock_path.clone()` goes away: `lock_key` borrows, and `lock_path` still
moves into the `spawn_blocking` closure at `lock.rs:116` unchanged.

FIND (count 1 in `crates/cyrup-config/src/lock.rs`):

```rust
        let in_process = CONFIG_LOCK_HANDLE
            .guard(lock_path.clone(), token)
            .await
            .map_err(|_| ConfigError::Cancelled)?;
```

REPLACE WITH:

```rust
        // Layer 1 keys on the RESOLVED sidecar path; layer 2 opens the caller's own spelling. They
        // must stay separate: `flock` is inode-based, so the raw spelling reaches the same lock,
        // while `ConfigError::Io` / `ConfigError::Lock` keep naming the path the operator typed.
        let in_process = CONFIG_LOCK_HANDLE
            .guard(lock_key(&lock_path), token)
            .await
            .map_err(|_| ConfigError::Cancelled)?;
```

That single substitution is the whole behavioural change. Do **not** substitute the resolved key for
`lock_path` inside `open_and_try_lock` — see §6's hard constraint.

### 5.4 `crates/cyrup-config/src/lock.rs` — the `FileLock` type doc

The paragraph to change is `lock.rs:53-56`. The blocking-vs-polling paragraph that follows it
(`:58-73`) belongs to the HIGH sibling and must survive verbatim.

FIND (count 1 in `crates/cyrup-config/src/lock.rs`):

```rust
/// Two layers. In-process contention — several tasks in one cyrup process reaching the same config
/// file — queues on a per-path async mutex: fair, cancel-aware, no syscall and no polling. That
/// admits at most one task per path per process, so this process presents at most one waiter to the
/// cross-process lock.
```

REPLACE WITH:

```rust
/// Two layers. In-process contention — several tasks in one cyrup process reaching the same config
/// file — queues on a per-key async mutex, the key being the sidecar path made lexically absolute
/// by [`lock_key`]: fair, cancel-aware, no syscall and no polling. Every spelling of one file that
/// does not differ through a symlink hashes to one entry, so that admits at most one task per file
/// per process, and this process presents at most one waiter to the cross-process lock.
///
/// The one alias layer 1 does not merge is a **symlink**: two such spellings are two keys, hence two
/// layer-2 waiters from this process on one inode. It costs an extra round trip, never correctness —
/// layer 2 locks the **inode**, so it excludes both aliases whichever name reached it. Upstream's
/// `mkdir` lock is path-keyed and does not, which is why this residue is affordable here and would
/// not have been in pi. Resolving it away would mean a `realpath`, and [`lock_key`] documents why
/// this domain must not take one.
```

Neither paragraph of the replacement mentions `flock`-vs-poll, `spawn_blocking`, cancellation or the
blocking pool, so the `/// **That bound holds only because the layer-2 wait lives inside …` paragraph
below it is untouched and stays exactly where it is.

`auth.rs:307-308`'s "keys per FILE" claim becomes *more* true after this and needs no edit.

### 5.5 `crates/cyrup-core/src/keyed_lock.rs` — the contract

Replace the flat obligation with one that states the real requirement and hands the choice back to
the domain. `cyrup-core` cannot depend on either consumer crate, so the two domains are named in
plain code spans, never as intra-doc links.

FIND (count 1 in `crates/cyrup-core/src/keyed_lock.rs`):

```rust
//! Keying is the caller's job too. This module never touches the filesystem (see the crate docs:
//! no I/O), so a domain that keys on resolved paths resolves them itself before calling
//! [`KeyedLocks::guard`].
```

REPLACE WITH:

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

## 6. Interaction with the sibling findings (do not edit their files)

**The queue has moved since this spec was first written. Both siblings are settled.**

- [`HIGH-dropped-acquire-future-detaches-blocking-flock-task`](../done/2026-08-23-00-08/HIGH-dropped-acquire-future-detaches-blocking-flock-task.md)
  has **already landed** — it now lives in `.flux/done/2026-08-23-00-08/`, and the tree confirms it:
  `lock.rs` imports `fs4::{FileExt, TryLockError}` (`:13`), has `FIRST_RETRY` / `MAX_RETRY` (`:39`,
  `:48`), a `tokio::pin!`ed `cancelled()` plus a `biased` `select!` retry loop inside `acquire`
  (`:122-145`), and the helpers are `open_and_try_lock` (`:156`) and `try_lock` (`:176`). The
  blocking `FileExt::lock` and the old `open_and_lock` are gone.
- [`MEDIUM-cancel-token-does-not-reach-the-cross-process-flock-wait`](./MEDIUM-cancel-token-does-not-reach-the-cross-process-flock-wait.md)
  is **void by its own terms** and now opens with a "QUEUE RESOLUTION — READ BEFORE EXECUTING / This
  spec is VOID as written" banner. Do not apply it.

So there is no contradiction left to route around; this task is a straight edit against the tree as
it stands. What remains true from the original interaction analysis:

- **The code edit is still one argument.** `acquire` keeps the same two statements in the same
  order — `CONFIG_LOCK_HANDLE.guard(…, token).await`, then a `spawn_blocking` that borrows
  `lock_path`. §5.3 changes only the first argument of `guard`. `lock_path` moves into the *first*
  `spawn_blocking` (`lock.rs:116`) and later retries carry only `file`; `lock_key(&lock_path)`
  borrows before that move, so it compiles unchanged.
- **What this task does NOT fix.** A dropped or cancelled acquire is a *lifetime* defect; this is a
  *key-space* defect. Neither implies the other: two symlink spellings are still two waiters after
  this change. What this removes is one of the two independent ways the "at most one waiter"
  sentence could be false; the HIGH sibling removed the other.
- **Hard constraint: `open_and_try_lock` keeps receiving `lock_path`, the caller's raw spelling —
  never `lock_key`'s output.** `flock` is inode-based, so the raw spelling reaches the same lock, and
  keeping it is what stops a resolved (or, on Windows, `\\?\`-verbatim) form from leaking into
  `ConfigError::Io { path }` / `ConfigError::Lock { path }` and being shown to an operator who never
  typed it. The resolved value exists only as a hash key and must never be rendered.

## 7. Explicitly not in scope

- **Do not canonicalize.** §3 and §4 are the reasons; re-adding a `realpath` to this crate also
  re-opens the `\\?\` divergence `ConfigDirs::resolve` documents at length (`env.rs:214-236`).
- **Do not change `ConfigDirs::resolve`** to `resolvePath(agent_dir)` for pi parity. `agent_dir`
  flows into session directory names, the `"cwd"` field in every session header, and rendered
  `<project_instructions path="…">` — absolutizing it changes user-visible output far beyond the
  lock, and would not cover `cyrup-ext-subagents`' externally-built paths anyway. The lock is the
  choke point every caller already passes through; fix it there.
- **Do not unify the two domains onto one keying function.** `cyrup-core` cannot do I/O, and the two
  domains are correct to differ. The contract sentence is what must change.
- **Do not touch `lock_path_for`.** It stays a pure lexical `<name>.lock` transform with no doc
  comment of its own; resolution is a separate, separately-documented step.
- **Do not touch `cyrup-session/src/git_paths.rs`'s private `lexically_normalize`** (`:110`). It is a
  different function in a different crate, cited in §3 only as precedent.
- **Do not touch `auth.rs`.** Its "keys per FILE" comment (`:307-308`) is strengthened, not
  invalidated, by this change.
- **No tests, no benchmarks, no new documentation files.** Another team owns those. The only doc
  changes here are the rustdoc / `//!` blocks named in §5.

## 8. Definition of done

No tests are written and no git command is used. Every item is checkable with a read or a `rg`.

1. **`crates/cyrup-config/src/paths.rs`** — `rg -n 'pub\(crate\) fn lexically_normalize' crates/cyrup-config/src/paths.rs`
   returns exactly one line, and §5.1's three-line `pub(crate)` rationale paragraph sits directly
   above it. The function body is byte-identical to what it was.
   `rg -n 'lexically_normalize' crates/cyrup-config/src/lib.rs` returns nothing.
2. **`crates/cyrup-config/src/lock.rs` — the helper** — `rg -n 'fn lock_key' crates/cyrup-config/src/lock.rs`
   returns exactly one line, and the function above it is still `lock_path_for`, unchanged and still
   undocumented. The doc block on `lock_key` matches §5.2, including the `settings-manager.ts:192-196`
   and `utils/paths.ts:81-85` citations.
3. **`crates/cyrup-config/src/lock.rs` — the call site** —
   `rg -F 'lock_path.clone()' crates/cyrup-config/src/lock.rs` finds nothing, and
   `rg -F --count-matches '.guard(lock_key(&lock_path), token)' crates/cyrup-config/src/lock.rs`
   prints `1`.
4. **The raw spelling still reaches layer 2** — `rg -n 'open_and_try_lock' crates/cyrup-config/src/lock.rs`
   shows the call site still passing `&lock_path`, and `rg -n 'lock_key' crates/cyrup-config/src/lock.rs`
   shows `lock_key` appearing only in its own definition, in doc comments, and in the one `guard(...)`
   call — never inside `open_and_try_lock`, `try_lock`, or any `ConfigError` construction.
5. **`crates/cyrup-config/src/lock.rs` — the type doc** — the `FileLock` doc says "per-key async
   mutex", names `lock_key` as an intra-doc link, and carries §5.4's symlink-residue paragraph. The
   paragraph beginning `/// **That bound holds only because the layer-2 wait lives inside` is still
   present, verbatim — this task overwrote none of the HIGH sibling's text.
6. **`crates/cyrup-core/src/keyed_lock.rs`** — the module doc matches §5.5, and
   `rg -n 'keys on resolved paths' crates/` returns nothing: no sentence in the tree still implies
   both domains resolve the same way.
7. **No stale invariant claim survives** — `rg -n 'one task per path|per path per process' crates/`
   returns nothing, and `rg -n 'keys per FILE' crates/` still returns only
   `crates/cyrup-config/src/auth.rs:308`, which is now strictly accurate.
8. **Public surface unchanged** — the `pub use paths::{…}` block at `crates/cyrup-config/src/lib.rs:67-69`
   is byte-identical to before, and neither `lock_key` nor `lexically_normalize` is `pub`.
9. **It builds and is idiomatic** — `cargo clippy -p cyrup-config -p cyrup-core --all-targets` is
   clean (the workspace denies `unwrap_used`, `expect_used`, `panic`, `indexing_slicing`; `lock_key`
   uses none of them).
10. **No NEW formatting debt.** `rustfmt --edition 2024 --check` on `crates/cyrup-config/src/lock.rs`
    and `crates/cyrup-config/src/paths.rs` reports **zero** diffs, and on
    `crates/cyrup-core/src/keyed_lock.rs` reports **exactly the three pre-existing hunks** that are
    already there before this task (a struct literal in `KeyedLocks::guard`, a `remove_if` chain in
    `KeyedGuard::drop`, and one more in the same drop path — all in code §5.5 does not touch; they
    belong to [`LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines`](./LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md),
    not to this task). Verified by simulation: applying all five §5 edits changes that count from 3
    to 3. Do not "fix" them here, and do not run `cargo fmt` without `--check` on that file.
11. **No test suite run.** Tests, benchmarks and standalone docs are another team's step; nothing in
    this task adds or edits any `#[test]`, `#[tokio::test]`, `benches/` entry, or `.md` under
    `spec/`.

---

## 9. QA verdict — 2026-08-23 07:54 (stage: qa, PASS, 9/10)

**The defect is fixed.** Layer 1 now keys on a lexically absolute sidecar path
(`lock.rs:280` `lock_key`, called at `lock.rs:127`), layer 2 still opens the caller's raw spelling
(`open_and_try_lock(&owned_target, &lock_path)`), and `keyed_lock.rs`'s module doc no longer asserts
a single flat notion of "resolved" for both domains.

### Verified against source and vendored upstream (settled — do not re-derive)

- All 10 checkable DoD items pass. `lock_path.clone()` is gone;
  `.guard(lock_key(&lock_path), token)` occurs once; `lock_key` appears only in its definition, two
  doc mentions and that one call — never in `open_and_try_lock`, `try_lock` or a `ConfigError`.
- `lexically_normalize` is `pub(crate)` with the rationale paragraph, body intact, still used by
  `resolve_path_from_base_with_home`; not exported from `lib.rs`; the `pub use paths::{…}` block is
  unchanged. No public surface moved.
- Every upstream citation in the new prose was checked against `tmp/pi` and is exact:
  `settings-manager.ts:206`, `trust-manager.ts:145`, `auth-storage.ts:56` are literally the only
  three `lockfile.lockSync` sites and all three pass `{ realpath: false }`; the constructor at
  `settings-manager.ts:192-196` is `resolvePath(cwd)` / `resolvePath(agentDir)`; `resolvePath` is at
  `utils/paths.ts:81-85` and is `nodeResolvePath`, no `realpathSync`; `getMutationQueueKey` is at
  `file-mutation-queue.ts:16-26` and does `await realpath`.
- In-tree claims check out: `env.rs:214+` is the SESS-036 `canonicalize`/`\\?\` note (cited by name,
  not line); `paths.rs:24-26` really says "this is NOT `resolve`"; `CYRUP_AGENT_DIR` is the real env
  name; `open_and_try_lock` really is what runs `ensure_dir` + `create(true)`, so the ENOENT
  argument against `realpath` holds.
- `rustfmt --edition 2024 --check` reports **zero** diffs on all three files — including
  `keyed_lock.rs`, where DoD 10 expected three pre-existing hunks (a sibling cleared them). Better
  than specified, not a defect.
- `cargo clippy -p cyrup-config -p cyrup-core --all-targets` emits nothing for `lock.rs`,
  `paths.rs` or `keyed_lock.rs`. `cargo doc -p cyrup-config --no-deps` is warning-free; the public
  `FileLock` doc linking private `lock_key` is fine because the workspace sets
  `private_intra_doc_links = "allow"` (root `Cargo.toml:114`).
- DoD 7's grep is not clean: `crates/cyrup-ext-subagents/src/discovery/settings_write.rs:276` still
  reads "Layer 1 admits one task per path per process". That line was written by the sibling
  `LOW-rewritten-concurrency-test-doc-misstates-which-lock-it-covers` (which prescribes it verbatim
  at its `:335`) after this spec's grep was taken, it sits in a file this task is forbidden to edit,
  and it is TRUE as written — that test uses one absolute tempdir path, and the sentence draws no
  "one waiter" inference. Not this task's debt.

### Notes carried forward (non-blocking, no rework required)

1. `lock.rs:58-59` — "The one alias layer 1 does not merge is a **symlink**" is the headline case,
   not the whole class: hardlinks, bind mounts and case-variant spellings on a case-insensitive
   filesystem (APFS, NTFS) also produce two keys for one file. The stated consequence is unchanged
   for every member of that class — layer 2 is `flock` on the inode, so it is an extra round trip
   and never a correctness loss — so no reader is misled about behaviour. If this text is ever
   revisited, widen the noun rather than the argument.
2. `lock.rs:296-297` — "the `open(2)` in `open_and_try_lock` is about to fail on the same relative
   path" is true for the unlinked-cwd case but not for a `getcwd` that fails with `EACCES` on an
   ancestor, where relative opens still succeed. The fallback itself is safe (it degrades to the
   pre-change key). Exotic enough to leave.
