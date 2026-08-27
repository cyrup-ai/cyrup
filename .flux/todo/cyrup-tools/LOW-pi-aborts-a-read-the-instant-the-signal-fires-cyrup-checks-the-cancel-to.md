---
title: Pi aborts a read the instant the signal fires; cyrup checks the cancel token exactly once and never during the file read
priority: LOW
tool: read
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: aug
status: done
updated: 2026-08-27
---

# Pi aborts a read the instant the signal fires; cyrup checks the cancel token exactly once and never during the file read

## Core objective

`read` must observe cancellation **continuously**, not once. In pi the abort listener is armed for
the whole lifetime of the call, so the promise rejects with `Operation aborted` at *any* instant the
signal fires — before the path is resolved, while `ops.readFile` is in flight, while the image is
being processed, and after the output text has been built but before it is resolved. In cyrup the
token is consulted exactly once, at
[`read.rs:137`](../../../crates/cyrup-tools/src/tools/read.rs), between the `R_OK` precheck and
`self.fs.read(&abs)` — every other moment of the call is blind to it.

Reaching parity means: (1) an already-fired token short-circuits **before any I/O**, (2) the file
read is *raced* against the token and observes it *during* the transfer, (3) image processing is
raced against the token, and (4) no `Ok(ToolResult)` is ever produced after the token has fired.

This task shares one cancellation adapter with its sibling
[*Cancellation is only observed between candidate files, not during a file's search*](./LOW-cancellation-is-only-observed-between-candidate-files-not-during-a-file.md)
(`grep`). The adapter is specified in full below and is introduced by **whichever of the two lands
first**; the second consumes it unchanged.

## What pi does — verified

[`read.ts`](../../../tmp/pi/packages/coding-agent/src/core/tools/read.ts), inside the `new Promise`
body of `execute`:

| line | code | meaning |
| --- | --- | --- |
| `:232-235` | `if (signal?.aborted) { reject(new Error("Operation aborted")); return; }` | an already-fired signal rejects **before `resolveReadPathAsync`, before `ops.access`, before any syscall** |
| `:237-241` | `const onAbort = () => { aborted = true; reject(new Error("Operation aborted")); }; signal?.addEventListener("abort", onAbort, { once: true });` | the listener is live for the **entire** call — it rejects the outer promise while `ops.readFile` / `processImage` are still pending |
| `:245-246` | `const absolutePath = await resolveReadPathAsync(path, cwd); if (aborted) return;` | guard after path resolution |
| `:248-249` | `await ops.access(absolutePath); if (aborted) return;` | guard after the readability precheck |
| `:256` / `:273` | `const buffer = await ops.readFile(absolutePath);` | image branch / text branch read — **both covered by the listener, not by a guard** |
| `:325-327` | `if (aborted) return; signal?.removeEventListener("abort", onAbort); resolve({ content, details });` | the single common guard before `resolve` — it dominates **all** result shapes (image ok, image failed, first-line-exceeds, truncated, user-limited, plain) |
| `:329-330` | `signal?.removeEventListener("abort", onAbort); if (!aborted) reject(error);` | an abort already rejected; a later real error must not re-reject |

The literal is `"Operation aborted"`, which
[`error::aborted()`](../../../crates/cyrup-tools/src/error.rs) (error.rs:117-119) already produces.

## What cyrup-tools does today — verified

[`read.rs`](../../../crates/cyrup-tools/src/tools/read.rs) — `cancel` appears in exactly two places
in the whole file: the `execute` parameter at `:98` and one test at `:137`.

```rust
// read.rs:135-143 — the ONLY cancellation observation in the tool
        self.fs.access(&abs, crate::ops::Access::Read).await?;

        if cancel.is_cancelled() {
            return Err(error::aborted());
        }

        // Read once through the (remote-aware) seam, then decide text-vs-image by MAGIC BYTES — Pi
        // sniffs the file header (read.ts:243 → mime.ts), not the extension.
        let bytes = self.fs.read(&abs).await?;
```

Consequences, each verified against the source:

* **No entry guard.** With an already-cancelled token, `execute` still deserializes, runs the whole
  macOS-variant probe loop (`read.rs:109-125`, one `FsOps::access(Exists)` per candidate) and the
  `R_OK` `access` before it reaches `:137`. pi rejects at `read.ts:232` before any of that.
* **The read cannot be interrupted.** `FsOps::read` takes no token
  ([`ops/mod.rs:323`](../../../crates/cyrup-tools/src/ops/mod.rs)), and neither `LocalFs::read`
  (`tokio::fs::read`, [`ops/local/fs.rs:63-67`](../../../crates/cyrup-tools/src/ops/local/fs.rs))
  nor the [`ProtectedFs`](../../../crates/cyrup-tools/src/isolation/protected.rs) (`:102-104`) /
  [`TraversalFs`](../../../crates/cyrup-tools/src/isolation/traversal.rs) (`:89-92`) decorators can
  observe one. There is no `select!` anywhere in `read.rs`.
* **No guard on the image path.** `read_image` (`read.rs:287-291`) takes no token, and
  `image_proc::process_image` (`read.rs:422-427`) is a **synchronous, CPU-bound decode + resize
  ladder invoked inline on the async worker** (`read.rs:306-311`) — it neither observes a cancel nor
  yields the runtime thread while a 4.5 MB JPEG ladder runs.
* **No guard before any `Ok`.** The three success returns — `read.rs:220` (first-line-exceeds),
  `read.rs:273` (text), and the four inside `read_image` (`read.rs:326`, `:347`, `:373`) — all
  produce a successful `ToolResult` regardless of the token, where pi's `read.ts:325` refuses.
* **Nothing above can rescue it.** [`exec.rs`](../../../crates/cyrup-agent/src/agent/run/tools/exec.rs)
  awaits `tool.execute` to completion in both runtimes: the parallel path drives it inside a
  `JoinSet` task (`:129-137`) and the sequential path's `tokio::select!` (`:269-287`) races only the
  update channel `urx.recv()` against `exec` — never the token. `is_cancelled()` is consulted only
  *between* calls, at `:90` and `:328`.
  [`wrap_registered_tool`](../../../crates/cyrup-ext/src/wrapper.rs) (`:146`) plainly forwards.
* **Every sibling already has the capability.** Biased `select!` on `cancel.cancelled()` in
  [`find.rs:173-184`](../../../crates/cyrup-tools/src/tools/find.rs) and
  [`grep.rs:391-395`](../../../crates/cyrup-tools/src/tools/grep.rs); post-mutation rechecks in
  [`write.rs:119-121`](../../../crates/cyrup-tools/src/tools/write.rs) and
  [`edit.rs:280-282`](../../../crates/cyrup-tools/src/tools/edit.rs) (mirroring pi's
  `throwIfAborted()` at `write.ts:224` / `edit.ts:352`). `read` simply never got it.

## User-visible impact

Cancelling (Esc / interrupt) during a read of a very large file — or any read over a slow remote
`FsOps` backend, where the whole transfer happens inside one `await` — returns `Operation aborted`
promptly in pi, whereas cyrup finishes the read, finishes the image decode, and returns the full
successful result. The cancelled turn therefore carries the file's content (and its token cost) into
the transcript instead of the abort. The run still ends as `StopReason::Aborted`, so the divergence
is the tool result, not the run outcome.

---

# Required implementation

Three files change. No dependency changes: `tokio` (with `rt` for `spawn_blocking`) and
`tokio_util`'s `CancellationToken` (re-exported as
[`cyrup_core::CancelToken`](../../../crates/cyrup-core/src/cancel.rs)) are already in use.

## 1. New shared adapter — `crates/cyrup-tools/src/ops/cancel_read.rs`

The single cancel-aware reader for the crate. `read` drains a whole file through it; `grep` wraps
its `search_reader` input in it. It exists in `ops/` rather than in either tool because it is a
property of the [`FsOps`](../../../crates/cyrup-tools/src/ops/mod.rs) seam's blocking `read_stream`
handle, and because two tools consume it.

Create the file with exactly this content:

```rust
//! Cancellation for the blocking side of the [`FsOps`](super::FsOps) seam.
//!
//! [`FsOps::read_stream`](super::FsOps::read_stream) hands back a blocking [`std::io::Read`] that
//! callers drive from `spawn_blocking`. Once that blocking task is running, a
//! [`CancelToken`] is invisible to it: `spawn_blocking` tasks cannot be aborted, and dropping the
//! `JoinHandle` only detaches. This wrapper is the observation point — it consults the token
//! before every delegated `read`, so a cancelled task unwinds within ONE buffer fill instead of
//! at the end of the file.
//!
//! This is the Rust stand-in for pi's `signal.addEventListener("abort", …, {once:true})`: pi's
//! reads live in libuv and the listener rejects the awaiting promise the instant the signal fires
//! (`read.ts:237-241`, `grep.ts:246-250` kills the ripgrep child outright). The async half of that
//! — returning to the caller immediately — is a `tokio::select!` at the call site; this type is the
//! other half, which stops the work itself.

use cyrup_core::CancelToken;
use std::io::{self, Read};

/// Granularity at which [`read_to_end_cancellable`] observes the token: one 64 KiB buffer fill.
///
/// Deliberately NOT `Read::read_to_end`: its `default_read_to_end` grows the destination
/// geometrically and issues correspondingly huge single `read` calls, so the token would be
/// consulted a handful of times over a multi-GB file. A fixed buffer makes the abort latency a
/// function of device throughput, not of file size.
const CHUNK: usize = 64 * 1024;

/// Payload carried by the [`io::Error`] a [`CancelRead`] returns once its token has fired.
///
/// A dedicated type rather than a sentinel `ErrorKind`, for two reasons. `ErrorKind::Interrupted`
/// is unusable: every std read loop (`read_to_end`, `io::copy`) RETRIES it, so a cancel signalled
/// that way spins forever. And `ErrorKind::Other` alone is indistinguishable from a genuine
/// backend failure, which callers must map to a real I/O error message rather than to
/// `Operation aborted`. Downcasting a marker type keeps the two apart with no string matching.
#[derive(Debug)]
pub(crate) struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Same literal pi throws, so the message survives even if a caller forgets to translate.
        f.write_str("Operation aborted")
    }
}

impl std::error::Error for Cancelled {}

/// True when `err` (or anything in its `source()` chain) is the [`Cancelled`] marker.
///
/// The chain walk matters because `grep_searcher::Searcher::search_reader` may return the sink's
/// error wrapped rather than verbatim.
pub(crate) fn is_cancelled_io(err: &io::Error) -> bool {
    let mut cur: Option<&(dyn std::error::Error + 'static)> =
        err.get_ref().map(|e| e as &(dyn std::error::Error + 'static));
    while let Some(e) = cur {
        if e.is::<Cancelled>() {
            return true;
        }
        cur = e.source();
    }
    false
}

/// A blocking reader that fails with [`Cancelled`] once `cancel` fires.
pub(crate) struct CancelRead<R> {
    inner: R,
    cancel: CancelToken,
}

impl<R: Read> CancelRead<R> {
    pub(crate) fn new(inner: R, cancel: CancelToken) -> Self {
        Self { inner, cancel }
    }
}

impl<R: Read> Read for CancelRead<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // Checked BEFORE delegating: an already-fired token must never start another syscall, and
        // a zero-length `buf` must not be able to mask the cancel behind an `Ok(0)`.
        if self.cancel.is_cancelled() {
            return Err(io::Error::other(Cancelled));
        }
        self.inner.read(buf)
    }
}

/// Drain `reader` to a `Vec`, observing `cancel` every [`CHUNK`] bytes.
///
/// Returns `Err` carrying [`Cancelled`] (test it with [`is_cancelled_io`]) when the token fires
/// mid-transfer; the partial buffer is dropped, exactly as pi discards a partially-read file when
/// the promise rejects.
pub(crate) fn read_to_end_cancellable<R: Read>(
    reader: R,
    cancel: &CancelToken,
) -> io::Result<Vec<u8>> {
    let mut src = CancelRead::new(reader, cancel.clone());
    let mut out: Vec<u8> = Vec::new();
    let mut buf = vec![0u8; CHUNK];
    loop {
        match src.read(&mut buf) {
            Ok(0) => return Ok(out),
            Ok(n) => out.extend_from_slice(&buf[..n]),
            // A REAL `EINTR`, not our cancel — the marker uses `ErrorKind::Other` precisely so
            // this retry cannot swallow it. The next iteration re-tests the token first.
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
}
```

## 2. Wire the module — `crates/cyrup-tools/src/ops/mod.rs`

Current (`ops/mod.rs:9-20`):

```rust
pub mod local;
pub mod shell;

use cyrup_core::{CancelToken, EventStream, ToolError};
```

Replacement:

```rust
pub(crate) mod cancel_read;
pub mod local;
pub mod shell;

pub(crate) use cancel_read::{CancelRead, is_cancelled_io, read_to_end_cancellable};

use cyrup_core::{CancelToken, EventStream, ToolError};
```

The module is `pub(crate)`: it is an implementation detail of the two tools, not part of the seam
an extension re-targets, so it must not widen `cyrup-tools`' public API.

## 3. `crates/cyrup-tools/src/tools/read.rs`

### 3a. Import the helpers

Current (`read.rs:5`):

```rust
use crate::ops::FsOps;
```

Replacement:

```rust
use crate::ops::{FsOps, is_cancelled_io, read_to_end_cancellable};
```

### 3b. Entry guard — before argument parsing and before any I/O

pi's `read.ts:232-235` runs before `resolveReadPathAsync`, and pi never validates arguments at all,
so an already-fired signal *always* yields `Operation aborted` there. Placing the guard after
`serde_json::from_value` would let a malformed-argument error win that race.

Current (`read.rs:91-99`):

```rust
    async fn execute(
        &self,
        _call_id: ToolCallId,
        params: serde_json::Value,
        cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        let input: ReadInput =
            serde_json::from_value(params).map_err(|e| error::invalid(format!("read: {e}")))?;
```

Replacement:

```rust
    async fn execute(
        &self,
        _call_id: ToolCallId,
        params: serde_json::Value,
        cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        // Pi's FIRST statement inside the promise body (read.ts:232-235), ahead of
        // `resolveReadPathAsync` and ahead of any argument handling — pi performs no runtime
        // validation, so an already-fired signal there can never lose to a schema error. Placed
        // before `from_value` for the same reason: with a cancelled token the model must see
        // "Operation aborted", not `read: invalid type…`. Without this guard the tool ran the
        // entire macOS-variant `access(Exists)` probe loop below plus the `R_OK` check on a run
        // the user had already cancelled.
        if cancel.is_cancelled() {
            return Err(error::aborted());
        }

        let input: ReadInput =
            serde_json::from_value(params).map_err(|e| error::invalid(format!("read: {e}")))?;
```

### 3c. Guard after path resolution, and race the read

Current (`read.rs:133-143`):

```rust
        // `"{resolved path}: {io error}"` (ops/local/fs.rs), so propagating is enough.
        self.fs.access(&abs, crate::ops::Access::Read).await?;

        if cancel.is_cancelled() {
            return Err(error::aborted());
        }

        // Read once through the (remote-aware) seam, then decide text-vs-image by MAGIC BYTES — Pi
        // sniffs the file header (read.ts:243 → mime.ts), not the extension.
        let bytes = self.fs.read(&abs).await?;
```

Replacement:

```rust
        // `"{resolved path}: {io error}"` (ops/local/fs.rs), so propagating is enough.

        // Pi's guard between `resolveReadPathAsync` and `ops.access` (read.ts:246). cyrup's
        // resolution loop above is `candidates.len()` round-trips through the seam — one per
        // macOS filename variant — so on a remote backend it is real latency the token must be
        // able to cut. No per-candidate check is added inside the loop: pi's probes live inside
        // `resolveReadPathAsync` and are likewise unguarded, and this is a parity task.
        if cancel.is_cancelled() {
            return Err(error::aborted());
        }

        self.fs.access(&abs, crate::ops::Access::Read).await?;

        // Pi's guard between `ops.access` and `ops.readFile` (read.ts:249).
        if cancel.is_cancelled() {
            return Err(error::aborted());
        }

        // Read through the (remote-aware) seam, then decide text-vs-image by MAGIC BYTES — Pi
        // sniffs the file header (read.ts:243 → mime.ts), not the extension.
        let bytes = self.read_cancellable(&abs, &cancel).await?;
```

### 3d. The cancellable read itself

Add this method to the `impl ReadTool` block that already holds `read_image` (`read.rs:285`), above
`read_image`:

```rust
    /// `ops.readFile` (read.ts:256 / :273) with pi's abort listener attached.
    ///
    /// Two independent windows have to be covered, and they need different mechanisms.
    ///
    /// **Opening the stream** is where a backend that cannot stream does its entire transfer:
    /// [`FsOps::read_stream`]'s default body is `Cursor::new(self.read(path).await?)`
    /// (`ops/mod.rs:341-343`), so for a remote/RPC `FsOps` the whole file arrives inside this one
    /// `await`. Nothing inside it can be interrupted, so it is RACED — the caller returns
    /// immediately and the orphaned future is dropped, which is precisely pi's shape: the abort
    /// listener rejects the promise while libuv's read is still in flight and nobody cancels
    /// libuv either.
    ///
    /// **Draining the stream** is where a large local file spends its time, and there the work
    /// itself must stop, not merely be abandoned: `spawn_blocking` tasks cannot be aborted and
    /// dropping the `JoinHandle` only detaches, so a bare `select!` would leave a thread reading
    /// gigabytes into a `Vec` nobody will ever look at. [`read_to_end_cancellable`] observes the
    /// token every 64 KiB, so the detached task also unwinds on its own. The `select!` here is
    /// still required: it is what makes the CALLER return at once when a single buffer fill blocks
    /// on a slow device.
    ///
    /// The seam moves from [`FsOps::read`] to [`FsOps::read_stream`] and that is safe for every
    /// existing backend: `LocalFs` overrides it with a real `File` (`ops/local/fs.rs:73-80`), both
    /// isolation decorators forward it explicitly (`protected.rs:122-124`, `traversal.rs:104-107`,
    /// the latter re-applying `confine` on the forwarded path), and any other implementation
    /// inherits the default, which routes straight back through that implementation's own
    /// `FsOps::read` — so a backend, decorator or probe that overrides only `read` still sees this
    /// tool's traffic.
    async fn read_cancellable(
        &self,
        abs: &std::path::Path,
        cancel: &CancelToken,
    ) -> Result<Vec<u8>, ToolError> {
        // `biased;` on every race in this tool. Without it `select!` polls in RANDOM order, so a
        // token that is ALREADY cancelled loses to a ready I/O future half the time and the read
        // completes anyway — the sibling `find.rs:177-183` documents the same trap. Pi's abort is
        // deterministic on both edges, so the cancel arm must be polled first.
        let reader = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(error::aborted()),
            r = self.fs.read_stream(abs) => r?,
        };

        let token = cancel.clone();
        let drain = tokio::task::spawn_blocking(move || read_to_end_cancellable(reader, &token));

        let joined = tokio::select! {
            biased;
            // Dropping `drain` DETACHES the blocking task rather than killing it; that is
            // acceptable only because `read_to_end_cancellable` is watching the same token and
            // returns within one buffer fill.
            _ = cancel.cancelled() => return Err(error::aborted()),
            j = drain => j,
        };

        match joined {
            Ok(Ok(bytes)) => Ok(bytes),
            // The token fired between two buffer fills: report pi's abort, not a raw I/O error.
            Ok(Err(e)) if is_cancelled_io(&e) => Err(error::aborted()),
            // A genuine backend failure keeps the shape `LocalFs::read` produced before this
            // change: `"{resolved path}: {io error}"`, propagated uncaught exactly as pi's
            // uncaught `ops.readFile` rejection is (read.ts:321-324).
            Ok(Err(e)) => Err(error::io(&error::show(abs), &e)),
            Err(e) => Err(error::invalid(format!("read: {e}"))),
        }
    }
```

### 3e. Image branch — pass the token, guard the result

Current (`read.rs:152-153`):

```rust
        if let Some(mime) = crate::ops::ImageMime::from_file_head(&bytes) {
            return self.read_image(bytes, mime).await;
        }
```

Replacement:

```rust
        if let Some(mime) = crate::ops::ImageMime::from_file_head(&bytes) {
            let result = self.read_image(bytes, mime, &cancel).await?;
            // Pi's single common guard before `resolve` (read.ts:325) — it dominates BOTH image
            // result shapes (`ok` and `failed`), so a cancel landing during the decode/resize
            // ladder yields the abort rather than an image block.
            if cancel.is_cancelled() {
                return Err(error::aborted());
            }
            return Ok(result);
        }
```

Current (`read.rs:287-291`):

```rust
    async fn read_image(
        &self,
        bytes: Vec<u8>,
        mime: crate::ops::ImageMime,
    ) -> Result<ToolResult, ToolError> {
```

Replacement:

```rust
    async fn read_image(
        &self,
        bytes: Vec<u8>,
        mime: crate::ops::ImageMime,
        cancel: &CancelToken,
    ) -> Result<ToolResult, ToolError> {
```

Current (`read.rs:304-311`):

```rust
        #[cfg(feature = "inline-images")]
        {
            match image_proc::process_image(
                &bytes,
                mime,
                self.opts.max_image_dim,
                self.opts.auto_resize_images,
            ) {
```

Replacement:

```rust
        #[cfg(feature = "inline-images")]
        {
            // `processImage` is `await`ed in pi (read.ts:257) with the abort listener live, so an
            // abort during it rejects at once. Here it is a SYNCHRONOUS decode + EXIF + resize +
            // JPEG-quality ladder (`image_proc::process_image`, below) that was invoked inline on
            // the async worker: it neither observed the token nor yielded the thread, so a
            // multi-megapixel source stalled the whole runtime worker. Moving it onto the blocking
            // pool is what makes the race expressible at all; the `Processed` value it returns is
            // owned (`String`/`Vec<String>`), so it crosses the boundary unchanged.
            let max_dim = self.opts.max_image_dim;
            let auto_resize = self.opts.auto_resize_images;
            let processing =
                tokio::task::spawn_blocking(move || {
                    image_proc::process_image(&bytes, mime, max_dim, auto_resize)
                });
            let processed = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(error::aborted()),
                p = processing => p.map_err(|e| error::invalid(format!("read: {e}")))?,
            };
            match processed {
```

The `match` arms and both `Ok(ToolResult { … })` bodies inside them are unchanged; only the scrutinee
moves from the inline call to the joined `processed` binding, and `bytes` is now moved into the
blocking closure instead of borrowed.

Current (`read.rs:357-361`):

```rust
        #[cfg(not(feature = "inline-images"))]
        {
            // Image decoding is only compiled out under `--no-default-features`; the default build
            // always inlines. Surface the detected type + a build note (and the non-vision note).
            let mut note = format!(
```

Replacement:

```rust
        #[cfg(not(feature = "inline-images"))]
        {
            // No decode happens in this build, so there is no work to race — but pi's guard before
            // `resolve` (read.ts:325) still applies, and this keeps `cancel` used on both cfg arms
            // without an `allow` attribute.
            if cancel.is_cancelled() {
                return Err(error::aborted());
            }
            // Image decoding is only compiled out under `--no-default-features`; the default build
            // always inlines. Surface the detected type + a build note (and the non-vision note).
            let mut note = format!(
```

### 3f. Guard both text-branch success returns

pi's `read.ts:325` sits after the if/else that builds `outputText`, so it dominates the
first-line-exceeds shape as well as the truncated / user-limited / plain shapes.

Current (`read.rs:213-220`):

```rust
            let out = format!(
                "[Line {line_no} is {}, exceeds {} limit. Use bash: sed -n '{line_no}p' {} | head -c {}]",
                format_size(first_line_bytes),
                format_size(DEFAULT_MAX_BYTES),
                input.path,
                DEFAULT_MAX_BYTES,
            );
            return Ok(ToolResult {
```

Replacement:

```rust
            let out = format!(
                "[Line {line_no} is {}, exceeds {} limit. Use bash: sed -n '{line_no}p' {} | head -c {}]",
                format_size(first_line_bytes),
                format_size(DEFAULT_MAX_BYTES),
                input.path,
                DEFAULT_MAX_BYTES,
            );
            // Pi's common pre-`resolve` guard (read.ts:325) covers this shape too — the
            // firstLineExceedsLimit branch assigns `outputText` and falls THROUGH to it.
            if cancel.is_cancelled() {
                return Err(error::aborted());
            }
            return Ok(ToolResult {
```

Current (`read.rs:270-278`):

```rust
        } else {
            None
        };
        Ok(ToolResult {
            content: vec![Content::text(out)],
            details,
            terminate: false,
            ..Default::default()
        })
```

Replacement:

```rust
        } else {
            None
        };
        // Pi's `if (aborted) return;` immediately before `resolve` (read.ts:325). The slicing,
        // `truncate_head` and continuation-notice formatting above are all CPU work over a file
        // that may be tens of megabytes, so this is a real window, not a formality.
        if cancel.is_cancelled() {
            return Err(error::aborted());
        }
        Ok(ToolResult {
            content: vec![Content::text(out)],
            details,
            terminate: false,
            ..Default::default()
        })
```

## 4. Sibling consumption — `crates/cyrup-tools/src/tools/grep.rs`

Recorded here so the two tasks land ONE adapter rather than two. The `grep` task owns this edit; it
must use `crate::ops::CancelRead` and `crate::ops::is_cancelled_io` exactly as introduced above,
adding them to the existing `use crate::ops::{FsOps, WalkOpts};` at `grep.rs:6`, threading `cancel`
into `search_one` (`grep.rs:73`) and wrapping the `read_stream` handle before it reaches the
blocking searcher (`grep.rs:93`, `:132`):

```rust
        // grep.rs:132 — inside the existing `spawn_blocking`
        let reader = CancelRead::new(reader, token);
        match searcher.search_reader(&matcher_owned, reader, sink) {
            Err(e) if is_cancelled_io(&e) => return Err(()),   // caller maps to error::aborted()
            _ => {}
        }
```

`read` must not duplicate this logic under another name, and `grep` must not introduce a second
adapter.

## Files that change

| file | change |
| --- | --- |
| [`crates/cyrup-tools/src/ops/cancel_read.rs`](../../../crates/cyrup-tools/src/ops/) | **new** — `Cancelled`, `is_cancelled_io`, `CancelRead<R>`, `read_to_end_cancellable`, `CHUNK` |
| [`crates/cyrup-tools/src/ops/mod.rs`](../../../crates/cyrup-tools/src/ops/mod.rs) | declare `pub(crate) mod cancel_read;` and re-export the three items |
| [`crates/cyrup-tools/src/tools/read.rs`](../../../crates/cyrup-tools/src/tools/read.rs) | import the helpers; entry guard before `from_value`; guard after the variant-resolution loop; new `read_cancellable` replacing `self.fs.read(&abs)`; `read_image` takes `&CancelToken` and races `process_image` on the blocking pool; guards before all three text-branch/image-branch `Ok` returns |
| [`crates/cyrup-tools/src/tools/grep.rs`](../../../crates/cyrup-tools/src/tools/grep.rs) | *(sibling task)* consume `CancelRead` / `is_cancelled_io` — listed so no second adapter is written |

No `Cargo.toml` change. No public API change (`lib.rs`'s `pub use ops::{…}` list is untouched).

## Genuinely uncertain

* **`read_stream` on an unknown backend.** In-tree the switch is provably safe (see the rationale in
  §3d). An out-of-tree `FsOps` that overrides `read_stream` with something whose bytes differ from
  its `read` would change what `read` returns; nothing in the trait contract forbids that, and
  `DistinctStreamFs`
  ([`tests/isolation.rs:416-452`](../../../crates/cyrup-tools/src/tests/isolation.rs)) exists to
  exploit exactly that freedom — though it is only ever driven through the `stream_text` helper,
  never through `ReadTool`.
* **Abort latency floor on the local backend.** With `LocalFs`, a 64 KiB `read(2)` on a stalled
  device (a hung NFS mount, a spun-down disk) is uninterruptible; the caller still returns at once
  via the `select!`, but the detached blocking thread stays parked until that syscall returns. pi
  has the identical floor — libuv's read is not cancelled either.
* **Guard placement inside the variant loop.** Left out deliberately (parity), but on a remote
  backend with several macOS filename variants the loop is several round-trips during which the
  token is not observed. If that latency is judged unacceptable in practice it becomes a separate,
  declared `[CYRUP-DELTA]`, not a silent addition here.

## Definition of done

Observable behaviour that must hold afterwards:

1. `read` invoked with an already-cancelled token fails with `Operation aborted` **and performs no
   filesystem operation at all** — no `access`, no `read_stream` — even when the arguments are
   malformed (the abort wins over the deserialization error).
2. A token that fires while the file transfer is in flight makes `read` return `Operation aborted`
   without waiting for the transfer to finish, for both a slow `FsOps::read_stream` open (remote
   backend) and a large local file mid-drain.
3. A token that fires mid-drain also **stops the transfer**: the detached blocking task ceases
   reading within one 64 KiB buffer fill and does not accumulate the remainder of the file.
4. A genuine I/O failure during the drain still surfaces as `"{resolved path}: {io error}"` — the
   message `LocalFs::read` produced before this change — and is never reported as
   `Operation aborted`; conversely a cancellation is never reported as an I/O error.
5. A token that fires while an image is being decoded/resized makes `read` return
   `Operation aborted` instead of an image block, and the decode no longer occupies an async
   runtime worker thread while it runs.
6. No `Ok(ToolResult)` is ever returned by `read` after the token has fired: this holds for the
   image-ok, image-failed, first-line-exceeds, truncated, user-limited and plain result shapes
   alike.
7. With a token that never fires, every result — text window, continuation notices, truncation
   `details`, image note, image block, `Offset … is beyond end of file` and the propagated raw
   errno text from `access` — is byte-for-byte what it was before this change.
8. Exactly one cancel-aware reader exists in `crates/cyrup-tools`; `grep` and `read` share it.
9. Nothing pi lacks is introduced: the only new externally visible outcome is `Operation aborted`
   arriving where the tool previously returned a successful result on a cancelled run.
