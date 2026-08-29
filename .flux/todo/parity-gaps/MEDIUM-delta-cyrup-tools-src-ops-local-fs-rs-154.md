---
title: "CYRUP-DELTA capability gap at crates/cyrup-tools/src/ops/local/fs.rs:154"
priority: MEDIUM
crate: cyrup-tools
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: aug
status: done
updated: 2026-08-28 21:00
---

# Close the `FsOps::mkdir` gap — hoist parent-directory creation out of `write_in_place`

**Disposition: CLOSE.** This is a port, not a design question. pi's source settles every
open point the previous pass deferred; they are answered inline below and none of them
returns to the owner.

Baseline: [`./tmp/pi`](../../../tmp/pi) at **`e8682309`**,
[`packages/coding-agent/package.json`](../../../tmp/pi/packages/coding-agent/package.json)
version **0.84.3**. Every line number below was re-opened and re-read for this pass; the
previous augmentation's anchors into `ops/mod.rs` had drifted **+43** and its two
`tools/grep.rs` anchors by **+226** — see §7.

---

## 1. Objective

`FsOps` has one write seam; pi has two. Hoist `create_dir_all` out of
`LocalFs::write_in_place` into a new required `FsOps::mkdir`, and call it from
`WriteTool::execute` with pi's abort check between the two — restoring the injectable
seam, the abort window, and (as a free consequence) the property that `edit` can never
create a directory.

## 2. Upstream — verified at `e8682309`

[`packages/coding-agent/src/core/tools/write.ts`](../../../tmp/pi/packages/coding-agent/src/core/tools/write.ts):

| Line | Statement |
|---|---|
| :3 | `import { mkdir as fsMkdir, writeFile as fsWriteFile } from "fs/promises";` |
| :31–36 | `export interface WriteOperations { writeFile; mkdir }` — **two** members; `mkdir: (dir: string) => Promise<void>` at :35 |
| :38–41 | `const defaultWriteOperations` — `mkdir: (dir) => fsMkdir(dir, { recursive: true }).then(() => {})` at :40 |
| :43–46 | `export interface WriteToolOptions { operations?: WriteOperations }` |
| :191 | `const ops = options?.operations ?? defaultWriteOperations;` — resolved once, per tool definition |
| :208–209 | `const absolutePath = resolveToCwd(path, cwd); const dir = dirname(absolutePath);` |
| :210 | `return withFileMutationQueue(absolutePath, async () => {` — the mutation queue wraps **both** ops |
| :215–217 | `const throwIfAborted = () => { if (signal?.aborted) throw new Error("Operation aborted"); };` |
| :219 | `throwIfAborted();` — **check A** |
| :221 | `await ops.mkdir(dir);` |
| :222 | `throwIfAborted();` — **check B** |
| :225 | `await ops.writeFile(absolutePath, content);` |
| :226 | `throwIfAborted();` — **check C** |

`mkdir` is reached only through `ops.`, so a supplier passing
`{ ...defaultWriteOperations, mkdir: refuse }` intercepts directory creation without
touching the write. It is a **public export** of the extension API:
`type WriteOperations` at
[`src/index.ts:331`](../../../tmp/pi/packages/coding-agent/src/index.ts)
(re-exported from `core/tools/index.ts:77`).

**pi's `edit` has no mkdir at all.**
[`core/tools/edit.ts`](../../../tmp/pi/packages/coding-agent/src/core/tools/edit.ts):
`EditOperations` is `{ readFile, writeFile, access }` (:96–103), `defaultEditOperations`
(:105–109), and the body calls `ops.access` (:349), `ops.readFile` (:359),
`ops.writeFile` (:371) — never a mkdir. This is load-bearing for §4.

**`mkdir` receives the DIRECTORY, never the file.** `dirname(absolutePath)` at
write.ts:209. An injected op upstream cannot see the leaf name. §5.4 follows from this
and is therefore settled, not open.

## 3. cyrup today — verified

[`crates/cyrup-tools/src/ops/mod.rs`](../../../crates/cyrup-tools/src/ops/mod.rs):

- `#[async_trait::async_trait]` :436, `pub trait FsOps: Send + Sync` **:437**. Members:
  `read` :438, `read_stream` :456 (provided), `write_in_place` :480 (required),
  `access` :482, `metadata` :483, `read_dir` :484, `detect_image_mime` :487 (provided),
  `walk` :501. **No `mkdir`.**
- `pub struct Backend { pub fs: Arc<dyn FsOps>, … }` :684–687.
- `FsOps` is public: `pub use ops::{ …, FsOps, … }` at
  [`lib.rs:46-51`](../../../crates/cyrup-tools/src/lib.rs) (the name is on :47). The gap
  is a public-API capability difference, exactly as `WriteOperations` is upstream.

[`crates/cyrup-tools/src/ops/local/fs.rs`](../../../crates/cyrup-tools/src/ops/local/fs.rs):

- `impl FsOps for LocalFs` **:131**; the `[CYRUP-DELTA]` marker **:154–159**;
  `async fn write_in_place` **:166**; the folded creation **:179–183**:
  `if let Some(parent) = path.parent() && !parent.as_os_str().is_empty() { tokio::fs::create_dir_all(parent).await.map_err(|e| error::io_errno(&format!("mkdir {}", error::show(parent)), &e))? }`

[`crates/cyrup-tools/src/tools/write.rs`](../../../crates/cyrup-tools/src/tools/write.rs):
`resolve_to_cwd` :106, `self.locks.guard(&abs, &cancel)` :108, **check A** :109–111,
`self.fs.write_in_place(&abs, bytes)` :114, **check C** :125–127 (landed under TOOL-041).
**Check B does not exist.** `path::resolve_to_cwd`
([`tools/path.rs:330`](../../../crates/cyrup-tools/src/tools/path.rs)) always returns an
absolute path, so `abs.parent()` is always `Some` and non-empty.

## 4. The three observable divergences

1. **No injectable seam.** A backend supplier can override `write_in_place` but cannot
   intercept, refuse, or redirect directory creation. A read-only-audit or remote/SSH
   `FsOps` that in pi refuses `mkdir` while allowing writes has nowhere to hook.
2. **The abort window (pi's check B) is missing.** A cancel landing between directory
   creation and the write: pi leaves the parent directory present and the file **absent**;
   cyrup leaves both, because `create_dir_all` + `open`/`write`/`flush` are one
   uninterruptible call. Both report the same `"Operation aborted"`
   ([`error.rs:117`](../../../crates/cyrup-tools/src/error.rs)) — the tool result is
   already correct, the **side effect** is not.
3. **cyrup's `edit` creates directories; pi's cannot.** Not in the original record.
   [`tools/edit.rs:324`](../../../crates/cyrup-tools/src/tools/edit.rs) writes through
   `self.fs.write_in_place`, i.e. through the same folded `create_dir_all`. The marker
   argues the `access(&abs, Access::ReadWrite)` precheck at `edit.rs:289` makes this
   moot — true for `LocalFs`, false for any custom `FsOps` with a permissive `access`
   (virtual/remote FS, record-and-replay double), and TOCTOU-shaped regardless (the
   parent can vanish between :289 and :324). The hoist removes this for free.

## 5. Prescription — the single required path

### 5.1 New trait method

In [`ops/mod.rs`](../../../crates/cyrup-tools/src/ops/mod.rs), beside `write_in_place`
(:480):

```rust
/// Create `dir` and every missing ancestor — Pi's SECOND injected write op,
/// `defaultWriteOperations.mkdir` (write.ts:35, :40):
/// `fsMkdir(dir, { recursive: true })`. Recursive and idempotent: an existing
/// directory is success, not `EEXIST`. Receives the DIRECTORY, never the file —
/// Pi passes `dirname(absolutePath)` (write.ts:209/:221).
async fn mkdir(&self, dir: &Path) -> Result<(), ToolError>;
```

**Required, with no provided body.** Not a preference — the crate documents this exact
hazard twice, on the two decorators that must forward it:
[`isolation/protected.rs:106-121`](../../../crates/cyrup-tools/src/isolation/protected.rs)
and
[`isolation/traversal.rs:94-103`](../../../crates/cyrup-tools/src/isolation/traversal.rs)
both explain that a decorator dropping a **provided** method is *silently* wrong,
"because the trait default and a dropped delegation return the same thing". Here it
would be worse: a defaulted local `create_dir_all` inside a remote backend creates the
directory on the wrong machine. A required method turns every omission into a compile
error. The public-API break is contained — §5.3 proves no implementor exists outside
`cyrup-tools`.

### 5.2 Call sites

- **[`ops/local/fs.rs`](../../../crates/cyrup-tools/src/ops/local/fs.rs)** — delete the
  folded block at :179–183 from `write_in_place`; add `LocalFs::mkdir` carrying that body
  verbatim, including `error::io_errno(&format!("mkdir {}", …), &e)`, so the error shape
  is byte-identical to today's. Rewrite the `[CYRUP-DELTA]` at :154–159 into a plain
  parity note (`write_in_place` is now 1:1 with `fsWriteFile`); the marker **is deleted,
  not re-annotated**. While in that comment, re-base its stale `write.ts:32-35, :215-218`
  to `:31-36, :38-41, :221`, and the sibling `edit.ts:83-87` in the `write_in_place`
  doc at `ops/mod.rs:463-464` to `edit.ts:105-109` (§7).

- **[`tools/write.rs`](../../../crates/cyrup-tools/src/tools/write.rs)** — between check A
  (:109–111) and the write (:114), insert pi's :221–222 pair:

  ```rust
  // Pi `write.ts:221` — `await ops.mkdir(dirname(absolutePath))`, a SEPARATE injected
  // op, inside `withFileMutationQueue` (`:210`) and after the first abort check.
  if let Some(parent) = abs.parent()
      && !parent.as_os_str().is_empty()
  {
      self.fs.mkdir(parent).await?;
  }
  // Pi `write.ts:222` — check B. A cancel here must leave the directory created and
  // the file untouched.
  if cancel.is_cancelled() {
      return Err(error::aborted());
  }
  ```

  Placement is after `self.locks.guard(...)` at :108, matching pi's mkdir being *inside*
  `withFileMutationQueue` (write.ts:210). Do not move it above the guard —
  [`tests/mutation_lock_is_first_await.rs`](../../../crates/cyrup-tools/src/tests/mutation_lock_is_first_await.rs)
  exists to catch exactly that.

- **[`tools/edit.rs`](../../../crates/cyrup-tools/src/tools/edit.rs)** — **no change.**
  Adding no `mkdir` call here *is* the fix for divergence 3 and matches `EditOperations`
  (§2). Call it out explicitly in the commit message; it is a second behaviour change,
  not an incidental one.

### 5.3 Every implementor — 15, enumerated and re-verified

Production (3):

| # | Site | Symbol | Required `mkdir` body |
|---|---|---|---|
| 1 | [`ops/local/fs.rs:131`](../../../crates/cyrup-tools/src/ops/local/fs.rs) | `impl FsOps for LocalFs` | the real `create_dir_all`, moved out of :179–183 |
| 2 | [`isolation/protected.rs:101`](../../../crates/cyrup-tools/src/isolation/protected.rs) | `impl FsOps for ProtectedFs` | `self.deny_if_protected(dir)?; self.inner.mkdir(dir).await` — mirrors `write_in_place` at :126–129 |
| 3 | [`isolation/traversal.rs:88`](../../../crates/cyrup-tools/src/isolation/traversal.rs) | `impl FsOps for TraversalFs` | `let p = self.confine(dir)?; self.inner.mkdir(&p).await` — mirrors `write_in_place` at :109–112 |

`TraversalFs::confine` (:58) handles non-existent targets already — the symlink check is
skipped when `canonicalize` fails and the lexical check still applies (module doc,
`traversal.rs:11-13`) — so a not-yet-created parent needs no special case.

Test doubles (12) — each names every required method today, so each stops compiling until
`mkdir` is added. That is the point:

| # | Site | Symbol |
|---|---|---|
| 4 | `src/tests/find_abort.rs:72` | `AbortProbeFs` |
| 5 | `src/tests/tools.rs:519` | `MutexProbeFs` |
| 6 | `src/tests/tools.rs:1543` | `CountingFs` |
| 7 | `src/tests/cross_registry_mutation_lock.rs:85` | `SplitWriteFs` |
| 8 | `src/tests/isolation.rs:306` | `RecordingFs` (backend-swap probe) |
| 9 | `src/tests/isolation.rs:407` | `DistinctStreamFs` |
| 10 | `src/tests/mutation_lock_is_first_await.rs:57` | `CountingFs` |
| 11 | `src/tests/pi_tool_semantics.rs:232` | `CancelOnWrite` |
| 12 | `src/tests/pi_tool_semantics.rs:412` | `CountingWalk` |
| 13 | `src/tests/pi_tool_semantics.rs:589` | `StreamOnlySearch` |
| 14 | `src/tools/grep.rs:1080` | `FailSecondRead` (in-file `mod tests`) |
| 15 | `src/tools/grep.rs:1593` | `RecordingFs` (in-file `mod tests`) |

`grep -rn "impl FsOps for" crates/ xtask/` returns exactly these 15. Nothing under
`cyrup-agent`, `cyrup-ext`, `cyrup-ext-sdk`, `cyrup-sdk`, `cyrup-session-svc` or `xtask`
implements the trait. `Arc<dyn FsOps>` **holders** (`tools/read.rs:27`, `tools/find.rs:28`,
`tools/grep.rs:106`, `tools/edit.rs:29`, `tools/write.rs:19`, `ops/mod.rs:685`) need no
change.

Two doubles matter for the body you give them:
`mutation_lock_is_first_await.rs:57`'s `CountingFs` asserts **zero** seam calls at the
first poll — its `mkdir` must increment `calls` like every other method, and the assertion
still holds because the new call sits after `guard()`. `pi_tool_semantics.rs:232`'s
`CancelOnWrite` should forward `mkdir` to `self.inner` unchanged; it is the template for
§6.2's new double.

### 5.4 The `ProtectedFs` ordering residual — decided, not escalated

The deleted marker's rationale is *"folded in here … so the protected-path decorator still
gets exactly one chance to deny BEFORE any directory is created."* Under the fix:

- **Protected component anywhere in the path** (`.git/config` → parent `…/.git`):
  `ProtectedPaths::is_protected` (`protected.rs:55-64`) is a *component* match over the
  whole path, so `ProtectedFs::mkdir` denies and nothing is created. Invariant preserved.
- **Protected leaf under an unprotected, not-yet-existing parent**
  (`w/brand/new/dir/.env`): `w/brand/new/dir` is created, then `write_in_place` denies.
  Residue: an empty directory, no file.

**Take pi's `mkdir(dir)` shape and accept the residue.** This is not a judgement call:
pi passes `dirname(absolutePath)` (write.ts:209/:221), so an injected op upstream *cannot*
see the leaf, and pi with an equivalent refusing `writeFile` produces exactly this residue.
An `ensure_parent_dir(file_path)` seam would close one delta by opening a smaller one.
Additionally, pi has no `ProtectedFs` at all — upstream's protected-paths is an example
extension blocking at the `tool_call` event *before* `execute`
([`examples/extensions/protected-paths.ts:13-29`](../../../tmp/pi/packages/coding-agent/examples/extensions/protected-paths.ts));
cyrup's mirroring analogue `protected_path_rule`
([`isolation/policy.rs:196`](../../../crates/cyrup-tools/src/isolation/policy.rs)) also fires
pre-execute and blocks the whole call, and `ProtectedFs` is cyrup's extra ops-seam sibling,
**off by default** (`SessionConfig::protect_paths: false`,
[`cyrup-session-svc/src/builder.rs:250`](../../../crates/cyrup-session-svc/src/builder.rs),
applied at :873 — decorator order is `LocalFs` → `TraversalFs` → `ProtectedFs`). Record the
residue in a one-line comment on `ProtectedFs::mkdir`. Do not add a non-pi seam for it.

No existing test observes the residue: `protected_paths_block_writes_pass_reads`
(`tests/isolation.rs:97`) asserts `!cwd.join(".env").exists()` — the *file* — with the
parent already present, and its `.git/config` case is denied at `mkdir` under the fix
with the same `"protected"` substring in the message.

## 6. Guards

### 6.1 The seam guard — REQUIRED (this is the DoD test)

New in [`src/tests/isolation.rs`](../../../crates/cyrup-tools/src/tests/isolation.rs),
beside the decorator-delegation block (`DistinctStreamFs` at :404 and its test at :465
are the template):

`mkdir_is_an_interceptable_seam_a_backend_can_refuse` — an `FsOps` double
`RefusingMkdirFs { inner: LocalFs }` whose `mkdir` returns
`ToolError::new("read-only backend: mkdir refused")` and whose `write_in_place` forwards
to `LocalFs` unchanged. Drive `WriteTool::execute` with
`{"path": "new/dir/out.txt", "content": "x"}` into a fresh `tempdir`. Assert: the error
contains `mkdir refused`; `cwd.join("new")` does **not** exist; `cwd.join("new/dir/out.txt")`
does **not** exist.

**RED today for a structural reason that belongs in the test's doc comment: the double
cannot be written at all, because `FsOps` has no `mkdir` to override.** That is precisely
the capability the record says a supplier lacks.

Companion, same file: `mkdir_forwards_through_both_isolation_decorators` — wrap the
refusing double in `TraversalFs::new(…, root)` then `ProtectedFs::with_defaults(…)` (the
production order from `builder.rs:869-875`), run the same write, assert the refusal still
surfaces. This is the guard against the silent-omission hazard `protected.rs:106-121`
describes; without it a future decorator that drops `mkdir` is invisible.

### 6.2 The abort-window guard — REQUIRED

New in
[`src/tests/pi_tool_semantics.rs`](../../../crates/cyrup-tools/src/tests/pi_tool_semantics.rs),
modelled on `write_rechecks_cancellation_after_the_write_lands` (:265+) and its
`CancelOnWrite` double (:226–252):

`write_rechecks_cancellation_between_mkdir_and_the_write` — `CancelOnMkdir { inner: LocalFs,
cancel: CancelToken }` whose `mkdir` performs the real creation and then calls
`self.cancel.cancel()`. Execute `write` for `"new/dir/out.txt"`. Assert
`err.to_string() == "Operation aborted"`; `cwd.join("new/dir")` **is** a directory (pi does
not undo the mkdir); `cwd.join("new/dir/out.txt")` does **not** exist. Pins write.ts:222.
RED today — the file is written before any cancel is observed.

### 6.3 Must stay green

- `write_still_creates_new_files_and_parent_dirs` —
  [`tests/write_semantics.rs:318-340`](../../../crates/cyrup-tools/src/tests/write_semantics.rs).
  The end-to-end contract that `write` creates missing parents; it must pass unchanged
  after the hoist. Re-base its doc citation `write.ts:215` → `:221`.
- `write_rechecks_cancellation_after_the_write_lands` (`pi_tool_semantics.rs:265`) and
  `edit_rechecks_cancellation_after_the_write_lands`.
- `protected_paths_block_writes_pass_reads` (`tests/isolation.rs:97`),
  `protected_fs_is_fs_only_and_bash_is_never_covered` (:170),
  `backend_swap_retargets_tools_without_contract_change` (:334).
- `mutation_lock_is_first_await` — see §5.3.
- `tests/edit_preview_diff.rs`, `tests/tools.rs`, `tests/cross_registry_mutation_lock.rs`,
  `tests/find_abort.rs` — compile-only impact.

## 7. Citation drift found this pass (fix in the files you touch)

| Where | Says | Actually at `e8682309` / HEAD |
|---|---|---|
| `ops/local/fs.rs:154-159` (marker) | `write.ts:32-35, :215-218` | `:31-36` + `:38-41` (interface/defaults), `:221` (call) |
| `ops/mod.rs:463-464` (`write_in_place` doc) | `write.ts:32-35`, `edit.ts:83-87` | `write.ts:31-36`, `edit.ts:105-109` |
| previous augmentation of this task | `FsOps` at `mod.rs:394`, `write_in_place` at `:437`, `Backend` at `:641` | `:437`, `:480`, `:684` (drift +43) |
| previous augmentation of this task | `grep.rs:854` / `grep.rs:1367` doubles | `grep.rs:1080` / `grep.rs:1593` (drift +226) |
| `isolation/protected.rs:105`, `traversal.rs:96`, `grep.rs:1594` | `ops/mod.rs:329-334`, `:363-365`, `:412-414` | `read_stream` default `:456-458`, `detect_image_mime` `:487-491` |

The last row is pre-existing drift in files this change does **not** otherwise touch —
fix the `protected.rs` / `traversal.rs` ones while adding `mkdir` there (you are editing
those impls anyway); leave `grep.rs` to a separate sweep.

Do **not** run `cargo` while auditing this file; the workspace has ten siblings on a 7.7G
disk.

## 8. Definition of done

1. `FsOps::mkdir(&self, dir: &Path) -> Result<(), ToolError>` exists as a **required**
   method on `ops/mod.rs`'s trait; all 15 implementors provide one; `LocalFs::write_in_place`
   contains no `create_dir_all` and no `mkdir` error context.
2. `WriteTool::execute` calls `self.fs.mkdir(parent)` after `locks.guard(...)` and before
   `write_in_place`, with a `cancel.is_cancelled()` check between the two.
3. `EditTool::execute` calls no `mkdir` — `edit` can no longer create a directory.
4. `mkdir_is_an_interceptable_seam_a_backend_can_refuse` passes; it cannot even be written
   against the current trait.
5. `write_rechecks_cancellation_between_mkdir_and_the_write` passes.
6. The `[CYRUP-DELTA]` marker at `ops/local/fs.rs:154-159` is gone (deleted, not
   re-annotated), and the citations in §7 rows 1–2 are re-based to `e8682309`.
7. §6.3 stays green.
