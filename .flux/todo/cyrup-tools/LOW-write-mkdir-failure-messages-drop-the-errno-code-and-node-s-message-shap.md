---
title: Write/mkdir failure messages drop the errno code and Node's message shape
priority: LOW
tool: write
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: exec
status: in-progress
updated: 2026-08-27
---

# Write/mkdir failure messages drop the errno code and Node's message shape

## Core objective

Every failure that originates inside `LocalFs::write_in_place` must reach the model with the
**libuv errno name as the leading token** and with the **syscall token Node names**, exactly as
pi's uncaught Node `SystemError.message` does. This is a *message-shape* task on a path that is
already functionally correct: the write still fails, the failure still reaches the model as
`isError`, and the errno classes are still distinguishable from Rust's `Display`. What is missing
is the `EACCES` / `EROFS` / `EISDIR` / `ENOSPC` token the model reads, and the `mkdir` / `open` /
`write` verb that tells it *which* step failed.

The capability to emit that token **already exists in this crate** — `error::io_errno`
([error.rs:78-83](../../../crates/cyrup-tools/src/error.rs)) — and is wired to six call sites, none
of them on the write path. **This task is a re-wiring of an existing helper, not a new helper.**

---

## What pi actually emits (verified against source)

pi's write ops are raw Node calls with no error handling of any kind:

```ts
// tmp/pi/packages/coding-agent/src/core/tools/write.ts:38-41
const defaultWriteOperations: WriteOperations = {
	writeFile: (path, content) => fsWriteFile(path, content, "utf-8"),
	mkdir: (dir) => fsMkdir(dir, { recursive: true }).then(() => {}),
};
```

and inside `execute` they are awaited bare — no `try`, no re-wrap:

```ts
// write.ts:220-226
// Create parent directories if needed.
await ops.mkdir(dir);
throwIfAborted();

// Write the file contents.
await ops.writeFile(absolutePath, content);
throwIfAborted();
```

[write.ts:38-41, :220-226](../../../tmp/pi/packages/coding-agent/src/core/tools/write.ts)

`edit` does the same at its write edge — `await ops.writeFile(absolutePath, finalContent);`
([edit.ts:371](../../../tmp/pi/packages/coding-agent/src/core/tools/edit.ts)), with
`writeFile: (path, content) => fsWriteFile(path, content, "utf-8")`
([edit.ts:107](../../../tmp/pi/packages/coding-agent/src/core/tools/edit.ts)). Neither write is
inside `edit`'s `try/catch` — that `catch` wraps only `ops.access`
([edit.ts:348-355](../../../tmp/pi/packages/coding-agent/src/core/tools/edit.ts)).

The rejection travels out through `wrapToolDefinition`, which forwards `execute` unaltered
([tool-definition-wrapper.ts:17-18](../../../tmp/pi/packages/coding-agent/src/core/tools/tool-definition-wrapper.ts)),
and is caught by the agent loop, which surfaces `error.message` **verbatim**:

```ts
// tmp/pi/packages/agent/src/agent-loop.ts:701-707 — executePreparedToolCall
} catch (error) {
	acceptingUpdates = false;
	await Promise.all(updateEvents);
	return {
		result: createErrorToolResult(error instanceof Error ? error.message : String(error)),
		isError: true,
	};
}
```

with `createErrorToolResult` at
[agent-loop.ts:760-765](../../../tmp/pi/packages/agent/src/agent-loop.ts) putting that string
straight into `content[0].text`.

So the model-observed text is Node's raw `SystemError.message`, whose shape is
`` `${code}: ${uv_strerror}, ${syscall}` `` plus `` ` '${path}'` `` when the libuv request carries
a path:

| failing step | typical Node message |
| --- | --- |
| `mkdir` denied by parent perms | `EACCES: permission denied, mkdir '/a/b'` |
| `mkdir` under a non-directory | `ENOTDIR: not a directory, mkdir '/a/b'` |
| `mkdir` where a file already sits | `EEXIST: file already exists, mkdir '/a/b'` |
| `open` denied | `EACCES: permission denied, open '/x'` |
| `open` on a directory | `EISDIR: illegal operation on a directory, open '/x'` |
| `open` on a read-only mount | `EROFS: read-only file system, open '/x'` |
| `write(2)` out of space | `ENOSPC: no space left on device, write` |

Three tokens are load-bearing: **the errno code (leading)**, **the syscall verb**, and **the path**
(absent on the `write` request, which is why the last row has none).

---

## What cyrup-tools emits today (verified)

[`WriteTool::execute`](../../../crates/cyrup-tools/src/tools/write.rs) propagates with a bare `?`:

```rust
// crates/cyrup-tools/src/tools/write.rs:108
self.fs.write_in_place(&abs, bytes).await?;
```

[`EditTool::execute`](../../../crates/cyrup-tools/src/tools/edit.rs) does the same:

```rust
// crates/cyrup-tools/src/tools/edit.rs:274
self.fs.write_in_place(&abs, final_text.as_bytes()).await?;
```

Both match pi (uncaught). The message is therefore whatever `write_in_place` built, and the only
implementation of that method is `LocalFs`
([ops/local/fs.rs:97-121](../../../crates/cyrup-tools/src/ops/local/fs.rs)) — all four of its
error edges use the plain `error::io` helper, which prepends nothing:

```rust
// crates/cyrup-tools/src/ops/local/fs.rs:97-121 — CURRENT
async fn write_in_place(&self, path: &Path, bytes: &[u8]) -> Result<(), ToolError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| error::io(&format!("create dir {}", error::show(parent)), &e))?;
    }
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .await
        .map_err(|e| error::io(&format!("write {}", error::show(path)), &e))?;
    file.write_all(bytes)
        .await
        .map_err(|e| error::io(&format!("write {}", error::show(path)), &e))?;
    // `tokio::fs::File` buffers; flush pushes the bytes to the OS. Node's `writeFile` likewise
    // only loops `write(2)` and closes the fd — there is no `fsync` on either side.
    file.flush()
        .await
        .map_err(|e| error::io(&format!("write {}", error::show(path)), &e))?;
    Ok(())
}
```

`error::io` is a bare context join:

```rust
// crates/cyrup-tools/src/error.rs:21-24
/// Wrap a std::io error with context.
pub(crate) fn io(context: &str, e: &std::io::Error) -> ToolError {
    ToolError::new(format!("{context}: {e}"))
}
```

Producing, on a denied write: `write /x: Permission denied (os error 13)`. No errno token, and the
`open` edge and the `write_all`/`flush` edges are indistinguishable because they share the literal
context string `write {path}`.

Nothing downstream repairs this. `ToolError` is a flat `{ message: String }`
([cyrup-core/src/tool.rs:78-86](../../../crates/cyrup-core/src/tool.rs)); both `FsOps` decorators
delegate `write_in_place` unchanged —
[traversal.rs:109-112](../../../crates/cyrup-tools/src/isolation/traversal.rs) applies `confine`
and forwards, [protected.rs:126-129](../../../crates/cyrup-tools/src/isolation/protected.rs)
applies `deny_if_protected` and forwards — and `LocalFs` is the crate's only backend implementing
[`FsOps::write_in_place`](../../../crates/cyrup-tools/src/ops/mod.rs) (`ops/mod.rs:365`).

---

## The helper that already exists

```rust
// crates/cyrup-tools/src/error.rs:76-83 — quoted verbatim
/// [`io`] with Node's errno code prepended, so the code survives the flattening into
/// [`ToolError`]'s single `message` field. See [`errno_name`].
pub(crate) fn io_errno(context: &str, e: &std::io::Error) -> ToolError {
    match errno_name(e) {
        Some(code) => ToolError::new(format!("{code}: {context}: {e}")),
        None => io(context, e),
    }
}
```

It is backed by `errno_name` ([error.rs:38-74](../../../crates/cyrup-tools/src/error.rs)), which
maps the raw errno on unix and falls back to `ErrorKind` on Windows, returning `None` for anything
outside its table — so `io_errno` degrades to today's exact output rather than inventing a code.
Its table already covers every class this path can produce: `EACCES`, `EPERM`, `EROFS`, `EISDIR`,
`ENOTDIR`, `ENOSPC`, `EEXIST`, `ELOOP`, `ENAMETOOLONG`, `EMFILE`, `ENFILE`, `EIO`, `EBUSY`,
`EINVAL`, `ENOENT`. **No addition to that table is required.**

The wire shape `CODE: context: display` is a crate-wide contract: `errno_code_of`
([error.rs:100-111](../../../crates/cyrup-tools/src/error.rs)) recovers the code by splitting at the
first `": "` and validating an all-uppercase `E…` head, and `edit` reads it to render pi's
`Error code: ${error.code}` line ([edit.rs:239-245](../../../crates/cyrup-tools/src/tools/edit.rs),
mirroring [edit.ts:352-354](../../../tmp/pi/packages/coding-agent/src/core/tools/edit.ts)).

### Every existing call site of the errno family

| site | helper | why |
| --- | --- | --- |
| [fs.rs:48-52](../../../crates/cyrup-tools/src/ops/local/fs.rs) `windows_access_result` | `io_errno_code("EPERM", …)` | libuv fixes the code on win32 |
| [fs.rs:149-152](../../../crates/cyrup-tools/src/ops/local/fs.rs) `access` (unix) | `io_errno` | `edit` recovers the code from it |
| [fs.rs:165](../../../crates/cyrup-tools/src/ops/local/fs.rs) `access` (non-unix stat) | `io_errno` | same |
| [fs.rs:191](../../../crates/cyrup-tools/src/ops/local/fs.rs) `read_dir` open | `io_errno` | `ls`'s `Cannot read directory: ${e.message}` body |
| [fs.rs:203](../../../crates/cyrup-tools/src/ops/local/fs.rs) `read_dir` iterate | `io_errno` | same |
| [lock.rs:159](../../../crates/cyrup-tools/src/lock.rs) `FileMutationLocks::key` realpath | `io_errno` | pre-write realpath failures |

The lock's realpath keying ([lock.rs:153-161](../../../crates/cyrup-tools/src/lock.rs)) is the only
partial mitigation on the write path, and it is narrow by design: `is_missing_path_error`
([lock.rs:32-45](../../../crates/cyrup-tools/src/lock.rs)) swallows `ENOENT`/`ENOTDIR` (the
brand-new-file case, pi's `isMissingPathError`), and the guard runs *before* `write_in_place`, so
it never observes `EACCES`-on-file, `EISDIR`, `EROFS`, `EEXIST` or `ENOSPC` — all of which
originate inside `write_in_place`.

`error::io` stays in use after this change (`read` at fs.rs:66, `read_stream` at fs.rs:78,
`metadata` at fs.rs:173), so nothing goes dead.

---

## Required change

**One file changes: [`crates/cyrup-tools/src/ops/local/fs.rs`](../../../crates/cyrup-tools/src/ops/local/fs.rs).**
Nothing else in the workspace changes.

Two edits inside `LocalFs::write_in_place` (fs.rs:97-121), plus one doc line:

1. Swap `error::io` → `error::io_errno` on all four error edges (`create_dir_all`, `open`,
   `write_all`, `flush`).
2. Rename the context strings to the syscall token Node names, so the message carries pi's verb as
   well as pi's code: `create dir {parent}` → `mkdir {parent}`, and the *open* edge's `write {path}`
   → `open {path}`. The `write_all` and `flush` edges keep `write {path}` — both are `write(2)`
   draining, which is exactly the syscall Node reports there.

### Replacement body

```rust
// crates/cyrup-tools/src/ops/local/fs.rs:97-121 — REPLACEMENT
async fn write_in_place(&self, path: &Path, bytes: &[u8]) -> Result<(), ToolError> {
    // `io_errno`, not `io`, on every edge below. Pi's write ops are raw Node calls whose
    // rejections propagate uncaught out of `execute` (write.ts:221/225, edit.ts:371) and reach
    // the model as `error.message` verbatim (agent-loop.ts:701-707), and a Node `SystemError`
    // message ALWAYS leads with the libuv errno name — `EACCES: permission denied, open '/x'`,
    // `ENOSPC: no space left on device, write`. `ToolError` is flat, so the code has to ride as
    // the leading token of the message; that is precisely what `error::io_errno` builds, and it
    // is the same `CODE: context: display` shape `access`/`read_dir`/`lock` already emit. The
    // context is the SYSCALL Node names for each edge (`mkdir`, `open`, `write`), so the model
    // can tell a parent-creation failure from an open failure from a short write, which the
    // single shared `write {path}` context could not.
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| error::io_errno(&format!("mkdir {}", error::show(parent)), &e))?;
    }
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .await
        .map_err(|e| error::io_errno(&format!("open {}", error::show(path)), &e))?;
    file.write_all(bytes)
        .await
        .map_err(|e| error::io_errno(&format!("write {}", error::show(path)), &e))?;
    // `tokio::fs::File` buffers; flush pushes the bytes to the OS. Node's `writeFile` likewise
    // only loops `write(2)` and closes the fd — there is no `fsync` on either side.
    file.flush()
        .await
        .map_err(|e| error::io_errno(&format!("write {}", error::show(path)), &e))?;
    Ok(())
}
```

### Edge-by-edge diff

| edge | fs.rs line | CURRENT | REPLACEMENT |
| --- | --- | --- | --- |
| `create_dir_all` | 101-103 | `error::io(&format!("create dir {}", error::show(parent)), &e)` | `error::io_errno(&format!("mkdir {}", error::show(parent)), &e)` |
| `open` | 105-111 | `error::io(&format!("write {}", error::show(path)), &e)` | `error::io_errno(&format!("open {}", error::show(path)), &e)` |
| `write_all` | 112-114 | `error::io(&format!("write {}", error::show(path)), &e)` | `error::io_errno(&format!("write {}", error::show(path)), &e)` |
| `flush` | 117-119 | `error::io(&format!("write {}", error::show(path)), &e)` | `error::io_errno(&format!("write {}", error::show(path)), &e)` |

### Doc-comment correction in the same hunk

The method's doc opens with a stale upstream citation:

```rust
// crates/cyrup-tools/src/ops/local/fs.rs:82 — CURRENT
/// 1:1 with Pi's `fsWriteFile(path, content, "utf-8")` (write.ts:33 / edit.ts:85):
```

`write.ts:33` is the `WriteOperations.writeFile` *type* line and `edit.ts:85` is a field of
`EditToolDetails`. The real implementations are `write.ts:39` and `edit.ts:107`:

```rust
// crates/cyrup-tools/src/ops/local/fs.rs:82 — REPLACEMENT
/// 1:1 with Pi's `fsWriteFile(path, content, "utf-8")` (write.ts:39 / edit.ts:107):
```

---

## Resulting messages

| failing step | pi (Node) | cyrup after this change |
| --- | --- | --- |
| mkdir denied | `EACCES: permission denied, mkdir '/a/b'` | `EACCES: mkdir /a/b: Permission denied (os error 13)` |
| mkdir under a file | `ENOTDIR: not a directory, mkdir '/a/b'` | `ENOTDIR: mkdir /a/b: Not a directory (os error 20)` |
| mkdir where a file sits | `EEXIST: file already exists, mkdir '/a/b'` | `EEXIST: mkdir /a/b: File exists (os error 17)` |
| open denied | `EACCES: permission denied, open '/x'` | `EACCES: open /x: Permission denied (os error 13)` |
| open on a directory | `EISDIR: illegal operation on a directory, open '/x'` | `EISDIR: open /x: Is a directory (os error 21)` |
| open on read-only fs | `EROFS: read-only file system, open '/x'` | `EROFS: open /x: Read-only file system (os error 30)` |
| write out of space | `ENOSPC: no space left on device, write` | `ENOSPC: write /x: No space left on device (os error 28)` |

Every token pi emits — code, syscall verb, path — is present, with Rust's `Display` standing in
for libuv's English string and carrying the raw errno number as a bonus.

Downstream consequences, all verified as intended and non-breaking:

* `errno_code_of` ([error.rs:100-111](../../../crates/cyrup-tools/src/error.rs)) now returns
  `Some(code)` for a `write_in_place` failure. Only `edit`'s **access precheck**
  ([edit.rs:239-245](../../../crates/cyrup-tools/src/tools/edit.rs)) consumes it, and that arm
  never sees a `write_in_place` error — `edit`'s own write failure propagates bare at edit.rs:274,
  exactly as pi's does at edit.ts:371. So no rendering path changes shape.
* `FATAL_BASH_PATTERNS`
  ([cyrup-ext-subagents/src/exec/output.rs:422-432](../../../crates/cyrup-ext-subagents/src/exec/output.rs))
  is bash-output-only and matches `"permission denied"` case-insensitively; the Rust `Display` half
  of the message is unchanged, so its behaviour is unaffected either way.

---

## Explicitly out of scope

* **Do not build a uv `strerror` table.** Reproducing `permission denied, open '/x'` word-for-word
  would require an errno→English map and a second message shape inside a crate where five other
  call sites already emit `CODE: context: display`. `errno_name`'s doc
  ([error.rs:35-37](../../../crates/cyrup-tools/src/error.rs)) states the rule the crate follows:
  fall through rather than invent. Keep the one shape.
* **Do not add entries to `errno_name`.** Its table already covers every class reachable from
  `mkdir(2)`, `open(2)` and `write(2)` on these paths.
* **Do not touch the decorators or the tools.** `write.rs:108` and `edit.rs:274` propagate bare,
  matching pi; `traversal.rs:109-112` and `protected.rs:126-129` delegate unchanged, matching pi's
  `{ ...ops, writeFile }` spread.
* **Do not widen the lock's `is_missing_path_error`** ([lock.rs:32-45](../../../crates/cyrup-tools/src/lock.rs));
  its `ENOENT`/`ENOTDIR`-only catch is a literal port of pi's `isMissingPathError`.
* **Do not change any success text, the `create_dir_all`-before-open ordering, the write-through
  (no temp-file-and-rename) semantics, or the absence of `fsync`.** All three are deliberate parity
  decisions documented at [fs.rs:82-96](../../../crates/cyrup-tools/src/ops/local/fs.rs) and
  [ops/mod.rs:355-365](../../../crates/cyrup-tools/src/ops/mod.rs).

---

## Citation corrections applied to this file

The original audit text carried four off-by-a-few references, corrected above:

* the four `error::io` write edges are fs.rs **:103, :111, :114, :119** (not `:101-102, :106-107,
  :110-111, :115-116`);
* `io_errno`'s existing consumers are fs.rs **:149-152** and **:191** (not `:145-148, :181-183`);
* `io_errno` is defined at error.rs **:78-83**, `errno_name` at **:38-74**, `io_errno_code` at
  **:93-95**, `errno_code_of` at **:100-111**;
* pi's verbatim-`error.message` catch is `agent-loop.ts` **:701-707** in
  `executePreparedToolCall` (the `:661-665` catch belongs to `prepareToolCall`, which runs before
  `execute` and never sees a filesystem rejection); `createErrorToolResult` is at **:760-765**;
* `FATAL_BASH_PATTERNS` is `output.rs` **:422-432**.

`write.ts:39`, `write.ts:40`, `write.ts:221`, `write.ts:225`, `fs.rs:48`, `fs.rs:165`, `fs.rs:203`,
`lock.rs:159`, `fs.rs:97-121`, `traversal.rs:109-111`, `protected.rs:126-128`, `write.rs:108` and
`edit.rs:240` were all checked and are correct as written.

---

## Definition of done

Observable behaviour, all on `crates/cyrup-tools`:

1. A `write` (or `edit`) call whose parent-directory creation fails returns a tool error whose
   message **begins** with the libuv errno name followed by `": "`, then `mkdir `, then the parent
   path — e.g. `EACCES: mkdir /a/b: Permission denied (os error 13)`.
2. A `write` (or `edit`) call whose `open` fails returns a message beginning with the errno name,
   then `open `, then the target path — e.g. `EISDIR: open /x: Is a directory (os error 21)`,
   `EROFS: open /x: Read-only file system (os error 30)`.
3. A `write` (or `edit`) call that fails while draining bytes returns a message beginning with the
   errno name, then `write `, then the target path — e.g.
   `ENOSPC: write /x: No space left on device (os error 28)`.
4. The three cases are mutually distinguishable from the message alone; the previous shared
   `write {path}` context no longer appears on the `open` edge.
5. `error::errno_code_of` applied to any of those errors yields `Some("EACCES")` /
   `Some("EISDIR")` / `Some("EROFS")` / `Some("ENOSPC")` / `Some("ENOTDIR")` / `Some("EEXIST")`
   as appropriate — i.e. the crate-wide `CODE: context: display` contract holds on the write path
   as it already does for `access`, `read_dir` and the mutation lock.
6. An `io::Error` outside `errno_name`'s table still produces today's exact output (`context:
   display`, no leading token) — `io_errno` falls back to `io`, no code is invented.
7. `error::io` remains the helper on `read`, `read_stream` and `metadata`; no other file in the
   workspace is modified.
8. `write` and `edit` success output, ordering, write-through semantics and abort behaviour are
   byte-for-byte what they are today — this is a message-shape change only, and no behaviour pi
   lacks is introduced.
