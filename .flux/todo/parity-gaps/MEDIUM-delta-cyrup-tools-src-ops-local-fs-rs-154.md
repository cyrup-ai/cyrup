---
title: "CYRUP-DELTA capability gap at crates/cyrup-tools/src/ops/local/fs.rs:154"
priority: MEDIUM
crate: cyrup-tools
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: aug
status: done
updated: 2026-08-28 02:10
---

# Capability gap: `crates/cyrup-tools/src/ops/local/fs.rs:154`

Classified a **capability gap** — a caller can observe a difference — by the audit
that reviewed all 87 `CYRUP-DELTA` markers against pi at `e8682309`.

**This was never authorized as an accepted divergence.** The marker was written by an
agent. Nobody decided it was acceptable. It is filed here so it is a decision rather
than an artifact.

## What pi does

pi's write tool declares `WriteOperations { writeFile, mkdir }` (core/tools/write.ts:31-37) — TWO members — with `defaultWriteOperations` at :38-41 and `WriteToolOptions.operations` at :43-46. `WriteOperations` is a public export of the extension API (packages/coding-agent/src/index.ts:331). `execute` calls `await ops.mkdir(dir)` then `throwIfAborted()` then `await ops.writeFile(...)` (write.ts:221-225).

## What cyrup does

`FsOps` has no `mkdir` member at all (ops/mod.rs:395-458). `LocalFs::write_in_place` folds `tokio::fs::create_dir_all(parent)` into the write.

## What a caller sees

Two observable consequences. (1) A backend/extension supplier can override `writeFile` but cannot override, intercept, or suppress `mkdir` independently — a remote/SSH or read-only-audit backend that in pi could refuse directory creation while allowing writes has no seam in cyrup. (2) pi's abort check between mkdir and writeFile is gone: a write cancelled in that window leaves pi with the directory created and no file, cyrup with both done. Error contexts also differ (`mkdir <path>` vs pi's raw Node mkdir rejection).

## Decision required

One of:

1. **Close it** — bring cyrup to pi's behaviour.
2. **Accept it** — David explicitly accepts the divergence; the marker stays and is
   annotated as authorized, with the reason.
3. **Reshape it** — the divergence is right but the current form is wrong.

Do not silently keep option 2 by leaving the marker as-is; that is how this became a
backlog in the first place.

## Definition of done

1. The gap is closed, or the marker records an explicit authorized acceptance.
2. If closed, a test fails without the change.
3. No behaviour regression in the owning crate.

---

# AUGMENTATION (stage: aug — 2026-08-28)

Reference tree: `./tmp/pi` at **`e8682309`**, `packages/coding-agent/package.json` version
**0.84.3**. Every pi line number below was read at that checkout. No `git` was run; no source
was edited.

## 1. pi's exact call sequence at `e8682309` — verified

`packages/coding-agent/src/core/tools/write.ts`:

| Line | Symbol / statement |
|---|---|
| :3 | `import { mkdir as fsMkdir, writeFile as fsWriteFile } from "fs/promises";` |
| :31-36 | `export interface WriteOperations { writeFile; mkdir }` — **two** members |
| :35 | `mkdir: (dir: string) => Promise<void>;` |
| :38-41 | `const defaultWriteOperations: WriteOperations` — `mkdir: (dir) => fsMkdir(dir, { recursive: true }).then(() => {})` at :40 |
| :43-46 | `export interface WriteToolOptions { operations?: WriteOperations }` |
| :187-191 | `createWriteToolDefinition(…, options?: WriteToolOptions)`; `const ops = options?.operations ?? defaultWriteOperations;` at :191 |
| :209 | `const dir = dirname(absolutePath);` |
| :210 | `return withFileMutationQueue(absolutePath, async () => {` |
| :215-217 | `const throwIfAborted = (): void => { if (signal?.aborted) throw new Error("Operation aborted"); };` |
| :219 | `throwIfAborted();` — **check A** |
| :221 | `await ops.mkdir(dir);` |
| :222 | `throwIfAborted();` — **check B** |
| :225 | `await ops.writeFile(absolutePath, content);` |
| :226 | `throwIfAborted();` — **check C** |
| :272 | `export function createWriteTool(cwd, options?: WriteToolOptions)` |

**Answer to the question posed: yes.** `mkdir` is a distinct, separately-overridable member of
the injected `WriteOperations` record, resolved once at :191 and reached only through `ops.` — so
a supplier passing `{ writeFile: …, mkdir: … }` (or spreading `{ ...defaultWriteOperations,
mkdir: refuse }`) intercepts directory creation without touching the write, and vice versa. It is
a **public export** of the extension API: `type WriteOperations` at
`packages/coding-agent/src/index.ts:331` (re-exported from `./core/tools/index.ts`).

**`edit` has no `mkdir` at all.** `EditOperations` is `{ readFile, writeFile, access }` only
(`core/tools/edit.ts:96-103`), `defaultEditOperations` at :105-109, and the body calls
`ops.access` (:349), `ops.readFile` (:359), `ops.writeFile` (:371) — never a mkdir. **pi's `edit`
can never create a directory.** This matters below.

## 2. What cyrup's `FsOps` surface offers instead — verified

`crates/cyrup-tools/src/ops/mod.rs`:

- `pub trait FsOps` opens at **:394**; the trait's members are `read` (:396), `read_stream`
  (:414, provided), `write_in_place` (:437, **required**), `access` (:439), `metadata` (:440),
  `read_dir` (:441), `detect_image_mime` (:444, provided), `walk` (:457). **There is no `mkdir`.**
  Confirms the record's "`FsOps` has no `mkdir` member at all".
- `pub struct Backend { pub fs: Arc<dyn FsOps>, pub proc: Arc<dyn ProcOps> }` at :641-644.

`FsOps` **is** a public item of the crate — `pub use ops::{ …, FsOps, … }` at
`crates/cyrup-tools/src/lib.rs:46-52` — so "a third-party backend supplier" is a real, supported
surface here exactly as `WriteOperations` is upstream. The gap is therefore a genuine
public-API capability difference, not a private-detail difference.

The folding site is `crates/cyrup-tools/src/ops/local/fs.rs`:

- `impl FsOps for LocalFs` at **:131**
- the `[CYRUP-DELTA]` marker at **:154-159** (exact anchor for this task)
- `async fn write_in_place` at **:166**; the folded creation is at **:180-186**:
  `if let Some(parent) = path.parent() && !parent.as_os_str().is_empty() { tokio::fs::create_dir_all(parent).await.map_err(|e| error::io_errno(&format!("mkdir {}", error::show(parent)), &e))? }`

### Marker citation drift (finding, not a descope)

The marker cites `write.ts:32-35, :215-218`. At `e8682309` those are **:31-36 / :38-41** (the
interface and defaults) and **:221** (the call). The marker's numbers are v0.83.0-era. Same for
`edit.ts:83-87` cited in the `FsOps::write_in_place` doc at `ops/mod.rs:429-431` — at `e8682309`
that is `edit.ts:105-109`. Whatever disposition is chosen, these citations should be re-based to
the pinned baseline; a stale citation is how the next audit re-derives a wrong conclusion.

## 3. Consequence (2) — CONFIRMED, and stated in full

The record's text is **not** truncated in substance; the marker at `fs.rs:154-159` says nothing
about abort at all, so consequence (2) is the audit's own derivation. Read directly from source it
is confirmed and sharpens as follows.

pi runs **three** `throwIfAborted()` calls (A :219, B :222, C :226). cyrup's `write` runs **two**:

- `crates/cyrup-tools/src/tools/write.rs:110-112` — `if cancel.is_cancelled() { return Err(error::aborted()) }` immediately after `self.locks.guard(&abs, &cancel)` at :109. **This is pi's check A.**
- `crates/cyrup-tools/src/tools/write.rs:124-126` — the post-write recheck. **This is pi's check C** (already closed under TOOL-041; see `tests/pi_tool_semantics.rs:263-292`, `write_rechecks_cancellation_after_the_write_lands`).

**The missing one is exactly check B (write.ts:222)** — the window between directory creation and
the file write. Precise observable difference for a cancel landing in that window:

- pi: `ops.mkdir` has completed, `throwIfAborted` at :222 throws `Operation aborted`, `ops.writeFile` **never runs**. Disk: parent directory present, target file **absent**.
- cyrup: `create_dir_all` and the `open`/`write`/`flush` are one uninterruptible `write_in_place` call; the cancel is first observed at write.rs:124. Disk: parent directory present **and the file fully written**.

Both surface the same `ToolError` text (`error::aborted()` produces pi's `"Operation aborted"` —
`error.rs:117`), so the **error is identical and only the disk state differs**. That is a real,
caller-observable divergence but it is narrower than "the abort check is gone": the tool result is
already correct; the side effect is not.

The error-context half also confirms, with a nuance worth recording. cyrup emits
`error::io_errno("mkdir {parent}", e)` → `"EACCES: mkdir /x: Permission denied (os error 13)"`.
pi's `fsMkdir` rejection propagates uncaught out of `execute` and reaches the model as
`error.message` verbatim → `"EACCES: permission denied, mkdir '/x'"`. **Both lead with the libuv
errno name**, which is the property `write_in_place`'s own comment (`fs.rs:167-177`) was built to
preserve; the divergence is the middle clause only.

## 4. Third divergence found — `edit` creates directories where pi's `edit` cannot

Not in the record. `crates/cyrup-tools/src/tools/edit.rs:324` writes through
`self.fs.write_in_place(&abs, …)`, i.e. through the same folded `create_dir_all`. pi's `edit`
has **no** mkdir member at all (§1). The marker argues this is harmless because `edit` first runs
`self.fs.access(&abs, Access::ReadWrite)` at `edit.rs:289`, which proves the file — and therefore
its parent — exists. That argument holds for `LocalFs`, but it is an argument about the **default
backend only**: a custom `FsOps` whose `access` is permissive (a virtual/remote FS, or a
record-and-replay double) makes cyrup's `edit` a directory-creating tool that upstream's cannot
be. It is also TOCTOU-shaped — the parent can be removed between :289 and :324.

Hoisting the creation out of `write_in_place` (§5) removes this for free: `edit` would then never
create a directory, matching `EditOperations` exactly. This is a reason to prefer closure over
acceptance, and it should be stated in the fix's commit rather than discovered later.

## 5. Prescribed closure — CLOSE (option 1)

### 5.1 The new seam

Add to `FsOps` in `crates/cyrup-tools/src/ops/mod.rs`, beside `write_in_place`:

```rust
/// Create `dir` and every missing ancestor — pi's SECOND injected write op,
/// `defaultWriteOperations.mkdir` (`write.ts:35`, `:40`):
/// `fsMkdir(dir, { recursive: true })`. Recursive and idempotent: an existing
/// directory is success, not `EEXIST`.
async fn mkdir(&self, dir: &Path) -> Result<(), ToolError>;
```

**Required, with no provided body — deliberately.** A defaulted
`tokio::fs::create_dir_all(dir)` body would be the exact hazard this crate already documents on
`read_stream`: `isolation/protected.rs:104-118` and `isolation/traversal.rs:92-100` both carry a
doc-comment explaining that a decorator which forgets to forward a **provided** method is
**silently** wrong, because "the trait default and a dropped delegation return the same thing".
Worse here — a defaulted local `create_dir_all` inside a *remote* backend would create the
directory on the wrong machine. A required method turns every omission into a compile error.
Cost: this is a **breaking change to `cyrup-tools`' public API** (`FsOps` is `pub use`d at
`lib.rs:46-52`); any out-of-tree implementor must add one method. Flag in the changelog.

### 5.2 The call sites

- `crates/cyrup-tools/src/ops/local/fs.rs` — delete the folded block at :180-186 from
  `write_in_place`; add `LocalFs::mkdir` carrying that body verbatim (same
  `error::io_errno("mkdir {dir}", e)` context, so §3's error shape is unchanged). Rewrite the
  `[CYRUP-DELTA]` at :154-159 into a plain parity note (`1:1 with fsWriteFile`) — the marker
  **goes away**, it is not re-annotated.
- `crates/cyrup-tools/src/tools/write.rs` — between the guard/check-A at :109-112 and the write at
  :114, insert pi's :221-222 pair:
  ```rust
  if let Some(parent) = abs.parent() && !parent.as_os_str().is_empty() {
      self.fs.mkdir(parent).await?;
  }
  if cancel.is_cancelled() { return Err(error::aborted()); }   // pi write.ts:222 — check B
  ```
  (pi passes `dirname(absolutePath)` unconditionally; `absolutePath` is always absolute after
  `resolveToCwd`, so `dirname` is never empty and the guard is a Rust-side no-op that only
  protects a degenerate relative path.) This closes consequence (2) in the same edit.
- `crates/cyrup-tools/src/tools/edit.rs` — **no change**. Not adding a `mkdir` call here is the
  fix for §4, and matches `EditOperations`.

### 5.3 Every implementor that must be updated — 15 sites, enumerated

Production (3):

| # | File:line | Symbol | Required `mkdir` body |
|---|---|---|---|
| 1 | `crates/cyrup-tools/src/ops/local/fs.rs:131` | `impl FsOps for LocalFs` | the real `create_dir_all`, moved out of :180-186 |
| 2 | `crates/cyrup-tools/src/isolation/protected.rs:101` | `impl FsOps for ProtectedFs` | `self.deny_if_protected(dir)?; self.inner.mkdir(dir).await` — mirrors `write_in_place` at :126-129 |
| 3 | `crates/cyrup-tools/src/isolation/traversal.rs:88` | `impl FsOps for TraversalFs` | `let p = self.confine(dir)?; self.inner.mkdir(&p).await` — mirrors `write_in_place` at :109-112 |

Test doubles (12) — each currently compiles because it names every required method; each will
stop compiling until `mkdir` is added, which is the point:

| # | File:line | Symbol |
|---|---|---|
| 4 | `crates/cyrup-tools/src/tests/find_abort.rs:72` | `AbortProbeFs` |
| 5 | `crates/cyrup-tools/src/tests/tools.rs:519` | `MutexProbeFs` |
| 6 | `crates/cyrup-tools/src/tests/tools.rs:1543` | `CountingFs` |
| 7 | `crates/cyrup-tools/src/tests/cross_registry_mutation_lock.rs:85` | `SplitWriteFs` |
| 8 | `crates/cyrup-tools/src/tests/isolation.rs:306` | `RecordingFs` (backend-swap probe) |
| 9 | `crates/cyrup-tools/src/tests/isolation.rs:407` | `DistinctStreamFs` |
| 10 | `crates/cyrup-tools/src/tests/mutation_lock_is_first_await.rs:57` | `CountingFs` |
| 11 | `crates/cyrup-tools/src/tests/pi_tool_semantics.rs:232` | `CancelOnWrite` |
| 12 | `crates/cyrup-tools/src/tests/pi_tool_semantics.rs:412` | `CountingWalk` |
| 13 | `crates/cyrup-tools/src/tests/pi_tool_semantics.rs:589` | `StreamOnlySearch` |
| 14 | `crates/cyrup-tools/src/tools/grep.rs:854` | `FailSecondRead` (in-file `mod tests`) |
| 15 | `crates/cyrup-tools/src/tools/grep.rs:1367` | `RecordingFs` (in-file `mod tests`) |

No implementor exists outside `cyrup-tools` — `impl FsOps for` matches nothing under
`crates/cyrup-agent`, `crates/cyrup-ext`, `crates/cyrup-ext-sdk`, `crates/cyrup-sdk` or `xtask`.
Non-`impl` consumers (`Arc<dyn FsOps>` holders in `tools/read.rs:27`, `tools/find.rs:28`,
`tools/grep.rs:106`, `tools/edit.rs:29`, `tools/write.rs:19`, `ops/mod.rs:642`) need no change.

### 5.4 One design consequence the reviewer must see (ProtectedFs ordering)

The marker's stated rationale is *"folded in here … so the protected-path decorator still gets
exactly one chance to deny BEFORE any directory is created."* Under the fix that invariant
survives for every case where a **path component** is protected (`.git/config` → parent `/w/.git`
is itself protected → `mkdir` denied, nothing created), because `ProtectedPaths::is_protected`
(`protected.rs:56-64`) is a component match over the whole path.

It does **not** survive for one narrow shape: a protected **leaf name** under an unprotected,
not-yet-existing parent — `write` to `w/brand/new/dir/.env`. Today: denial, nothing created.
After: `w/brand/new/dir` is created, then `write_in_place` denies; no file, an empty directory.

Assessment, for David rather than settled by me:

- It matches pi's own ordering (mkdir precedes the write there too), and **pi has no
  `ProtectedFs`** — upstream's protected-paths mechanism is an example extension that blocks at
  the `tool_call` event *before* `execute` — `examples/extensions/protected-paths.ts:14-27`. cyrup's
  gate-level analogue, `protected_path_rule` (`isolation/policy.rs:196-205`), also fires
  pre-execute and blocks the whole call, so in the configuration that mirrors pi nothing is
  created at all. `ProtectedFs` is cyrup's **extra** ops-seam sibling (`protected.rs:8-11`) and is
  **off by default** (ADR-0003 D5/D6, quoted at `tests/isolation.rs:161-170`).
- No existing test breaks: `protected_paths_block_writes_pass_reads`
  (`tests/isolation.rs:96-155`) asserts `!cwd.join(".env").exists()` — the *file* — with the
  parent already present.
- If the empty-directory residue is judged unacceptable, the alternative is a
  `ensure_parent_dir(&self, file_path: &Path)` seam taking the **file** path so `ProtectedFs` can
  deny on the target. That buys the invariant at the cost of diverging from pi's `mkdir(dir)`
  shape — i.e. closing one delta by opening a smaller one. **Recommend pi's `mkdir(dir)` shape and
  a one-line note in `ProtectedFs::mkdir`; raised here so it is a decision, not an artifact.**

## 6. Tests

### 6.1 The interception guard (the new seam's reason to exist) — REQUIRED

New, in `crates/cyrup-tools/src/tests/isolation.rs` beside the existing decorator-delegation
tests (the `DistinctStreamFs` block at :400-540 is the template):

`mkdir_is_an_interceptable_seam_a_backend_can_refuse` — an `FsOps` double
(`RefusingMkdirFs { inner: LocalFs }`) whose `mkdir` returns
`error::denied("read-only backend: mkdir refused")` and whose `write_in_place` forwards to
`LocalFs` unchanged. Drive `WriteTool::execute` with `{"path": "new/dir/out.txt", "content": "x"}`
into a fresh `tempdir`. Assert: the error contains `mkdir refused`; `cwd.join("new")` does **not**
exist; `cwd.join("new/dir/out.txt")` does **not** exist. **RED today for a structural reason
worth stating in the test's doc comment: the double cannot even be written, because `FsOps` has no
`mkdir` to override** — that is precisely the capability the record says a supplier lacks. This is
DoD #2.

Companion, same file: `mkdir_forwards_through_both_isolation_decorators` — wrap the refusing
double in `TraversalFs::new(…, root)` and then `ProtectedFs::with_defaults(…)`, run the same
write, and assert the refusal still surfaces. This is the guard against the silent-omission hazard
`protected.rs:104-118` describes; without it a future decorator that drops `mkdir` is invisible.

### 6.2 The abort-window guard (consequence 2) — REQUIRED

New, in `crates/cyrup-tools/src/tests/pi_tool_semantics.rs`, directly modelled on
`write_rechecks_cancellation_after_the_write_lands` (:263-292) and its `CancelOnWrite` double
(:226-252):

`write_rechecks_cancellation_between_mkdir_and_the_write` — a `CancelOnMkdir { inner: LocalFs,
cancel: CancelToken }` whose `mkdir` performs the real creation and then calls
`self.cancel.cancel()`. Execute `write` for `"new/dir/out.txt"`. Assert:
`err.to_string() == "Operation aborted"`; `cwd.join("new/dir")` **is** a directory (pi does not
undo the mkdir); `cwd.join("new/dir/out.txt")` does **not** exist. Pins pi write.ts:222 exactly.
RED today (the file is written before any cancel is observed).

### 6.3 Regression guards that must stay green (DoD #3)

- `write_still_creates_new_files_and_parent_dirs` — `tests/write_semantics.rs:320-343`; the
  existing contract that `write` creates missing parents. Must pass unchanged after the hoist.
  Consider extending its doc comment's `write.ts:215` citation to `:221`.
- `write_rechecks_cancellation_after_the_write_lands` / `edit_rechecks_cancellation_after_the_write_lands` — `tests/pi_tool_semantics.rs:263+`.
- `protected_paths_block_writes_pass_reads` (`tests/isolation.rs:96`),
  `protected_fs_is_fs_only_and_bash_is_never_covered` (`tests/isolation.rs:168+`),
  `backend_swap_retargets_tools_without_contract_change` (`tests/isolation.rs:332+`).
- `mutation_lock_is_first_await` (`tests/mutation_lock_is_first_await.rs`) — the new `fs.mkdir`
  call sits *after* `self.locks.guard(...)` at `write.rs:109`, matching pi's placement inside
  `withFileMutationQueue` (write.ts:210). Verify this test's `CountingFs` counts the mkdir the way
  it counts the write.
- `crates/cyrup-tools/src/tests/edit_preview_diff.rs`, `tests/tools.rs`,
  `tests/cross_registry_mutation_lock.rs` — compile-only impact (new required method).

Do **not** run `cargo` while auditing this file; the workspace has ten siblings on a 7.7G disk.

## 7. Open questions for David

1. **The `ProtectedFs` ordering residual (§5.4).** pi's `mkdir(dir)` shape, accepting an empty
   directory created before a protected-leaf denial — or a non-pi `ensure_parent_dir(file_path)`
   shape that preserves cyrup's zero-effect denial? Recommendation: pi's shape.
2. **`FsOps::mkdir` as a required method breaks `cyrup-tools`' public API** (§5.1). Confirm that
   is acceptable in this release, or state a deprecation path. A provided default is the only
   alternative and it reintroduces the silent-omission hazard the crate already documents twice.
3. **§4 — cyrup's `edit` can create directories, pi's cannot.** Closing this gap removes that
   silently. Confirm it should be called out in the commit as a second behaviour change rather
   than folded in unremarked.
4. **Marker citation re-basing (§2).** `write.ts:32-35 / :215-218` and `edit.ts:83-87` are
   v0.83.0 numbers surviving in source comments at `fs.rs:154-159` and `ops/mod.rs:429-431`.
   Re-base to `e8682309` as part of this change, or as a separate sweep across all remaining
   markers?

## 8. Recommendation

**CLOSE (option 1).** The seam is a public-API capability upstream exposes and cyrup does not; the
marker's own rationale (the `ProtectedFs` deny-before-create invariant) is preserved for every
protected-component case by giving `ProtectedFs::mkdir` the same guard as `ProtectedFs::write_in_place`,
and the same edit closes the abort-window divergence and the unrecorded `edit`-creates-directories
divergence at no extra cost. Nothing here is recorded as accepted or out of scope; §7 is for the
owner.
