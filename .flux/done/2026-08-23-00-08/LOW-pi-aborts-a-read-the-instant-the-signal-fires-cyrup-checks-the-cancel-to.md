---
title: Pi aborts a read the instant the signal fires; cyrup checks the cancel token exactly once and never during the file read
priority: LOW
tool: read
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: qa
status: completed
updated: 2026-08-27 14:06
---

# Pi aborts a read the instant the signal fires; cyrup checks the cancel token exactly once and never during the file read

## Core objective

`read` must observe cancellation **continuously**, not once. In pi the abort listener is armed for
the whole lifetime of the call, so the promise rejects with `Operation aborted` at *any* instant the
signal fires — before the path is resolved, while `ops.readFile` is in flight, while `processImage`
is running, and after the output text has been built but before it is resolved. In cyrup the token
is consulted exactly once, at [read.rs:137](../../../crates/cyrup-tools/src/tools/read.rs), between
the `R_OK` precheck and `self.fs.read(&abs)` — every other moment of the call is blind to it.

Parity means four things:

1. an already-fired token short-circuits **before any I/O and before argument validation**;
2. the file read is raced against the token *and* observes it **during** the transfer;
3. the image decode/resize is raced against the token;
4. no `Ok(ToolResult)` is ever produced after the token has fired — for **any** of the six result
   shapes.

## Shared adapter — reconciliation with the `grep` sibling

The sibling task
[*Cancellation is only observed between candidate files, not during a file's search*](./LOW-cancellation-is-only-observed-between-candidate-files-not-during-a-file.md)
needs the identical primitive: a `std::io::Read` that fails with a recognisable sentinel once the
token fires, because `spawn_blocking` tasks cannot be aborted from outside. That brief specifies it
in full — `Cancelled` (+ `Cancelled::err()` / `Cancelled::is()`) and `CancelReader<R>` — and places
it *inside* `grep.rs` as a private adapter, which was correct while `grep` was its only consumer.

It is no longer the only consumer. **The two tools MUST share one adapter**, so:

* The types keep the sibling's **exact names, exact bodies and exact rationale** — including the
  `ErrorKind::Other` (never `Interrupted`) requirement, which that brief traces to
  `encoding_rs_io-0.1.7 src/util.rs:230-241` retrying `Interrupted` forever inside
  `DecodeReaderBytes`' BOM sniff. That trap is not grep-specific: `std`'s own `read_to_end` and
  `io::copy` retry `Interrupted` too, so an `Interrupted` sentinel would spin in this tool as well.
* Their single home is a new module,
  `crates/cyrup-tools/src/ops/cancel_read.rs`, at `pub(crate)` visibility. `pub(crate)` is not the
  "do not make it public" the sibling rules out — the items stay invisible outside the crate and are
  not added to `lib.rs`'s `pub use ops::{…}` list; they simply stop being private to one tool.
* **Whichever task lands first creates the module.** If `grep` lands first with the items inside
  `grep.rs`, this task performs a mechanical hoist — move the two types verbatim into the new
  module, change `struct` → `pub(crate) struct` and `fn err`/`fn is`/`fn new` → `pub(crate) fn`, and
  replace the definitions in `grep.rs` with `use crate::ops::cancel_read::{CancelReader, Cancelled};`.
  Nothing in `grep`'s logic changes. `struct Aborted` stays local to `grep.rs`: it is that tool's
  blocking-outcome marker, not part of the adapter.
* Neither tool may introduce a second reader adapter under another name.

## What pi does — verified

[pi read.ts](../../../tmp/pi/packages/coding-agent/src/core/tools/read.ts), inside the `new Promise`
body of `execute`:

| line | code | meaning |
| --- | --- | --- |
| `:232-235` | `if (signal?.aborted) { reject(new Error("Operation aborted")); return; }` | an already-fired signal rejects **before `resolveReadPathAsync`, before `ops.access`, before any syscall** |
| `:237-241` | `const onAbort = () => { aborted = true; reject(new Error("Operation aborted")); }; signal?.addEventListener("abort", onAbort, { once: true });` | the listener is live for the **entire** call — it rejects the outer promise while `ops.readFile` / `processImage` are still pending |
| `:245-246` | `const absolutePath = await resolveReadPathAsync(path, cwd); if (aborted) return;` | guard after path resolution |
| `:248-249` | `await ops.access(absolutePath); if (aborted) return;` | guard after the readability precheck |
| `:256` / `:273` | `const buffer = await ops.readFile(absolutePath);` | image branch / text branch read — covered by the **listener**, not by a guard |
| `:257` | `const processed = await processImage(buffer, mimeType, { autoResizeImages });` | image work is `await`ed with the listener live |
| `:325-327` | `if (aborted) return; signal?.removeEventListener("abort", onAbort); resolve({ content, details });` | the single common guard before `resolve` — it dominates **all six** result shapes (image ok, image failed, first-line-exceeds, truncated, user-limited, plain) |
| `:329-330` | `signal?.removeEventListener("abort", onAbort); if (!aborted) reject(error);` | an abort already rejected; a later real error must not re-reject |

The literal is `"Operation aborted"`, which
[error::aborted()](../../../crates/cyrup-tools/src/error.rs) (error.rs:117-119) already produces.

## What cyrup-tools does today — verified

[read.rs](../../../crates/cyrup-tools/src/tools/read.rs) — `cancel` appears in exactly two places in
the whole file: the `execute` parameter at `:98` and one `is_cancelled()` call at `:137`.

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
  macOS-variant probe loop (read.rs:109-125, one `FsOps::access(Exists)` per candidate) and the
  `R_OK` `access` before it reaches `:137`. pi rejects at read.ts:232 before any of that.
* **The read cannot be interrupted.** `FsOps::read` takes no token
  ([ops/mod.rs:323](../../../crates/cyrup-tools/src/ops/mod.rs)), and neither `LocalFs::read`
  (`tokio::fs::read`, [ops/local/fs.rs:63-67](../../../crates/cyrup-tools/src/ops/local/fs.rs)) nor
  the [ProtectedFs](../../../crates/cyrup-tools/src/isolation/protected.rs) (`:102-104`) /
  [TraversalFs](../../../crates/cyrup-tools/src/isolation/traversal.rs) (`:89-92`) decorators can
  observe one. There is no `select!` and no `run_until_cancelled` anywhere in `read.rs`.
* **No guard on the image path.** `read_image` (read.rs:287-291) takes no token, and
  `image_proc::process_image` (read.rs:422-427) is a **synchronous, CPU-bound decode + EXIF +
  resize + JPEG-quality-ladder** invoked inline on the async worker (read.rs:306-311): it neither
  observes the token nor yields the runtime thread.
* **No guard before any `Ok`.** The success returns at read.rs:220 (first-line-exceeds), read.rs:273
  (text) and the three inside `read_image` (read.rs:326, `:347`, `:373`) all produce a successful
  `ToolResult` regardless of the token, where pi's read.ts:325 refuses.
* **Nothing above can rescue it.**
  [exec.rs](../../../crates/cyrup-agent/src/agent/run/tools/exec.rs) awaits `tool.execute` to
  completion in both runtimes: the parallel path drives it inside a `JoinSet` task (`:129-137`) and
  the sequential path's `tokio::select!` (`:269-287`) races only the update channel `urx.recv()`
  against `exec` — never the token. `is_cancelled()` is consulted only *between* calls, at `:90` and
  `:328`. [wrap_registered_tool](../../../crates/cyrup-ext/src/wrapper.rs) (`:146`) plainly
  forwards.
* **Every sibling already has the capability.** Biased `select!` on `cancel.cancelled()` in
  [find.rs:173-184](../../../crates/cyrup-tools/src/tools/find.rs) and
  [grep.rs:391-395](../../../crates/cyrup-tools/src/tools/grep.rs); post-mutation rechecks in
  [write.rs:119-121](../../../crates/cyrup-tools/src/tools/write.rs) and
  [edit.rs:280-282](../../../crates/cyrup-tools/src/tools/edit.rs), mirroring pi's `throwIfAborted()`
  at write.ts:224 / edit.ts:352. `read` simply never got it.

*(Citation corrections against the original audit note: the abort literals in `read.ts` are at
`:233` and `:239`, not `:226`; the guards are `:246`, `:249`, `:325`; the sequential `select!` in
`exec.rs` is at `:269-287`, not `:258-283`; the semantics test file is
`crates/cyrup-tools/src/tests/pi_tool_semantics.rs`, not `tests/pi_tool_semantics.rs`. Everything
else in that note checks out.)*

## User-visible impact

Cancelling (Esc / interrupt) during a read of a very large file — or any read over a slow remote
`FsOps` backend, where the whole transfer happens inside one `await` — returns `Operation aborted`
promptly in pi, whereas cyrup finishes the read, finishes the image decode, and returns the full
successful result. The cancelled turn therefore carries the file's content (and its token cost) into
the transcript instead of the abort. The run still ends as `StopReason::Aborted`, so the divergence
is the tool result, not the run outcome.

---

# Required implementation

Three files change (plus the two-line `use` swap in `grep.rs` if that task landed first). No
dependency changes: `tokio` (`rt` for `spawn_blocking`) and `tokio_util`'s `CancellationToken`
— re-exported as [cyrup_core::CancelToken](../../../crates/cyrup-core/src/cancel.rs), cancel.rs:9 —
are already in use.

## 1. Shared adapter — `crates/cyrup-tools/src/ops/cancel_read.rs`

New file. `Cancelled` and `CancelReader` are the sibling brief's types, moved here verbatim with
`pub(crate)` visibility; `read_to_end_cancellable` is this task's addition, built on top of them.

```rust
//! Cancellation for the blocking side of the [`FsOps`](super::FsOps) seam.
//!
//! [`FsOps::read_stream`](super::FsOps::read_stream) hands back a blocking [`std::io::Read`] that
//! callers drive from `spawn_blocking`. Once that blocking task is running a [`CancelToken`] is
//! invisible to it: `spawn_blocking` tasks cannot be aborted, and dropping the `JoinHandle` only
//! DETACHES — the thread keeps burning CPU on a file nobody will look at. So the abort has to be
//! PULLED IN through the one thing the blocking work keeps asking us for: bytes.
//!
//! This is the in-process stand-in for pi's `signal.addEventListener("abort", …, {once:true})`.
//! pi's reads live in libuv and the listener rejects the awaiting promise the instant the signal
//! fires (read.ts:237-241); its grep kills the ripgrep child outright (grep.ts:240-250). The async
//! half of that — returning to the CALLER at once — is `CancelToken::run_until_cancelled` at the
//! call site; this module is the other half, which stops the WORK.
//!
//! Shared by [`crate::tools::ReadTool`] (whole-file drain) and `grep` (`search_reader` input).

use cyrup_core::CancelToken;
use std::io::{self, Read};

/// Buffer size for [`read_to_end_cancellable`]: the granularity at which the token is observed.
///
/// Deliberately NOT `Read::read_to_end`. Its `default_read_to_end` grows the destination
/// geometrically and issues correspondingly huge single `read` calls, so the token would be
/// consulted a handful of times over a multi-GB file. A fixed buffer makes abort latency a function
/// of device throughput, not of file size. 64 KiB also matches `grep_searcher`'s
/// `DEFAULT_BUFFER_CAPACITY` (grep-searcher-0.1.16 `src/line_buffer.rs:6`), so both consumers of
/// this module abort on the same beat.
const CHUNK: usize = 64 * 1024;

/// The payload attached to the `io::Error` a cancelled [`CancelReader`] read — or a cancelled
/// `MatchSink` callback in `grep` — returns, so the caller can tell "the token fired mid-transfer"
/// apart from a genuine I/O failure. That distinction is load-bearing in BOTH consumers: in `read`
/// a real failure must keep its `"{path}: {io error}"` message, and in `grep` a real failure must
/// stay a silent skip, while a cancel must become [`crate::error::aborted`].
///
/// The kind is deliberately [`std::io::ErrorKind::Other`] and NEVER `Interrupted`. `Interrupted` is
/// the "retry me" kind: `std`'s own `read_to_end` / `io::copy` loop on it, and `grep`'s path adds a
/// second trap — `search_reader` wraps the reader in `encoding_rs_io`'s `DecodeReaderBytes`, whose
/// BOM sniff retries `Interrupted` inside a `while !buf.is_empty()` loop
/// (encoding_rs_io-0.1.7 `src/util.rs:230-241`). An `Interrupted` sentinel would spin forever
/// instead of aborting.
#[derive(Debug)]
pub(crate) struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Same wording as `error::aborted`, so a stray leak reads identically to pi's rejection.
        f.write_str("Operation aborted")
    }
}

impl std::error::Error for Cancelled {}

impl Cancelled {
    /// The `io::Error` carrying this marker. `grep_searcher`'s `SinkError for io::Error` forwards
    /// reader errors verbatim (`fn error_io(err: io::Error) -> io::Error { err }`,
    /// grep-searcher-0.1.16 `src/sink.rs:47-49`), so the boxed payload survives the trip out of
    /// `search_reader` unchanged; `read`'s drain loop returns it directly.
    pub(crate) fn err() -> io::Error {
        io::Error::other(Cancelled)
    }

    /// Recover the marker from an `io::Error`.
    pub(crate) fn is(e: &io::Error) -> bool {
        e.get_ref().is_some_and(|src| src.is::<Cancelled>())
    }
}

/// A [`std::io::Read`] adapter that fails with [`Cancelled`] the moment `cancel` fires.
pub(crate) struct CancelReader<R> {
    inner: R,
    cancel: CancelToken,
}

impl<R> CancelReader<R> {
    pub(crate) fn new(inner: R, cancel: CancelToken) -> Self {
        Self { inner, cancel }
    }
}

impl<R: Read> Read for CancelReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // Before: never start another chunk once the token has fired. This also means a
        // zero-length `buf` cannot mask the cancel behind an `Ok(0)`.
        if self.cancel.is_cancelled() {
            return Err(Cancelled::err());
        }
        let n = self.inner.read(buf)?;
        // After: a cancel that landed WHILE this read was parked — a slow disk, a network-backed
        // `FsOps`, a `read_stream` that is not a plain `File` — is observed now rather than one
        // chunk later. Dropping the bytes just read is sound because the transfer is being
        // abandoned: the caller returns `error::aborted()` and no partial result is ever emitted.
        if self.cancel.is_cancelled() {
            return Err(Cancelled::err());
        }
        Ok(n)
    }
}

/// Drain `reader` into a `Vec`, observing `cancel` every [`CHUNK`] bytes.
///
/// Returns an `Err` carrying [`Cancelled`] (test it with [`Cancelled::is`]) when the token fires
/// mid-transfer; the partial buffer is dropped, exactly as pi discards a partially-read file when
/// the promise rejects.
pub(crate) fn read_to_end_cancellable<R: Read>(
    reader: R,
    cancel: &CancelToken,
) -> io::Result<Vec<u8>> {
    let mut src = CancelReader::new(reader, cancel.clone());
    let mut out: Vec<u8> = Vec::new();
    let mut buf = vec![0u8; CHUNK];
    loop {
        match src.read(&mut buf) {
            Ok(0) => return Ok(out),
            Ok(n) => out.extend_from_slice(&buf[..n]),
            // A REAL `EINTR`, not our cancel — the marker uses `ErrorKind::Other` precisely so this
            // retry cannot swallow it. The next iteration re-tests the token first.
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
}
```

## 2. Wire the module — `crates/cyrup-tools/src/ops/mod.rs`

Current ([ops/mod.rs:9-12](../../../crates/cyrup-tools/src/ops/mod.rs)):

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

use cyrup_core::{CancelToken, EventStream, ToolError};
```

Nothing is added to the `pub use` block below it and nothing is added to `lib.rs`'s
`pub use ops::{…}` list: the adapter is crate-internal, not part of the seam an extension re-targets.

## 3. `crates/cyrup-tools/src/tools/read.rs`

### 3a. Import the helpers

Current (read.rs:5):

```rust
use crate::ops::FsOps;
```

Replacement:

```rust
use crate::ops::FsOps;
use crate::ops::cancel_read::{Cancelled, read_to_end_cancellable};
```

### 3b. Entry guard — before argument parsing and before any I/O

pi's read.ts:232-235 runs before `resolveReadPathAsync`, and pi performs **no runtime argument
validation at all**, so an already-fired signal there can never lose to a schema error. Placing the
guard after `serde_json::from_value` would let `read: invalid type…` win that race.

Current (read.rs:91-99):

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
        // `resolveReadPathAsync` and ahead of any argument handling — pi never validates tool
        // arguments, so an already-fired signal cannot lose to a schema error there. Hence before
        // `from_value` here too. Without this guard the tool ran the entire macOS-variant
        // `access(Exists)` probe loop below, plus the `R_OK` check, on a run the user had already
        // cancelled — on a remote `FsOps` that is several round-trips of pure waste.
        if cancel.is_cancelled() {
            return Err(error::aborted());
        }

        let input: ReadInput =
            serde_json::from_value(params).map_err(|e| error::invalid(format!("read: {e}")))?;
```

### 3c. Guard after path resolution; route the read through the cancellable helper

Current (read.rs:133-143):

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

        // Pi's guard between `resolveReadPathAsync` and `ops.access` (read.ts:246). The resolution
        // loop above is `candidates.len()` round-trips through the seam — one per macOS filename
        // variant — so on a remote backend it is real latency the token must be able to cut. No
        // per-candidate check is added INSIDE that loop: pi's probes live inside
        // `resolveReadPathAsync` and are equally unguarded, and this is a parity task.
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

Add this method to the `impl ReadTool` block that already holds `read_image` (read.rs:285),
immediately **above** `read_image`:

```rust
    /// `ops.readFile` (read.ts:256 / :273) with pi's abort listener attached.
    ///
    /// Two independent windows have to be covered, and they need different mechanisms.
    ///
    /// **Opening the stream** is where a backend that cannot stream does its ENTIRE transfer:
    /// [`FsOps::read_stream`]'s default body is `Cursor::new(self.read(path).await?)`
    /// (ops/mod.rs:341-343), so for a remote/RPC `FsOps` the whole file arrives inside this one
    /// `await`. Nothing inside it can be interrupted, so it is RACED and the orphaned future is
    /// dropped — precisely pi's shape, where the listener rejects the promise while libuv's read is
    /// still in flight and nobody cancels libuv either.
    ///
    /// **Draining the stream** is where a large local file spends its time, and there the work
    /// itself must stop rather than merely be abandoned: `spawn_blocking` tasks cannot be aborted
    /// and dropping the `JoinHandle` only detaches, so a bare race would leave a thread reading
    /// gigabytes into a `Vec` nobody will read. [`read_to_end_cancellable`] carries the token
    /// INSIDE the blocking closure and unwinds within one 64 KiB buffer fill. The outer race is
    /// still required: it is what makes the CALLER return at once when a single fill is parked on
    /// a slow device.
    ///
    /// `run_until_cancelled` rather than a `biased` `select!`: it short-circuits to `None` when the
    /// token is ALREADY cancelled (tokio-util 0.7.18 `sync/cancellation_token.rs:280-293`), which
    /// is the same determinism `find.rs:177-183` gets from `biased;`, and it is the idiom the grep
    /// sibling uses for the identical races.
    ///
    /// The seam moves from [`FsOps::read`] to [`FsOps::read_stream`], and that is safe for every
    /// backend: `LocalFs` overrides it with a real `File` (ops/local/fs.rs:73-80); both isolation
    /// decorators forward it explicitly (protected.rs:122-124, traversal.rs:104-107, the latter
    /// re-applying `confine` to the forwarded path); and any implementation that overrides only
    /// `read` inherits the default, which routes straight back through that implementation's own
    /// `read` — so recording/counting backends still see this tool's traffic.
    async fn read_cancellable(
        &self,
        abs: &std::path::Path,
        cancel: &CancelToken,
    ) -> Result<Vec<u8>, ToolError> {
        let Some(opened) = cancel.run_until_cancelled(self.fs.read_stream(abs)).await else {
            return Err(error::aborted());
        };
        // A failure to OPEN keeps propagating uncaught, exactly as `self.fs.read(&abs)?` did and as
        // pi's uncaught `ops.readFile` rejection does (read.ts:321-324).
        let reader = opened?;

        let token = cancel.clone();
        let drain = tokio::task::spawn_blocking(move || read_to_end_cancellable(reader, &token));

        // Dropping `drain` here DETACHES the blocking task rather than killing it; that is
        // acceptable only because the closure holds the same token and returns within one buffer
        // fill. Never weaken `read_to_end_cancellable` to rely on this race alone.
        let Some(joined) = cancel.run_until_cancelled(drain).await else {
            return Err(error::aborted());
        };

        match joined {
            Ok(Ok(bytes)) => Ok(bytes),
            // The token fired between two buffer fills: report pi's abort, not a raw I/O error.
            Ok(Err(e)) if Cancelled::is(&e) => Err(error::aborted()),
            // A genuine backend failure keeps the shape `LocalFs::read` produced before this
            // change: `"{resolved path}: {io error}"`.
            Ok(Err(e)) => Err(error::io(&error::show(abs), &e)),
            // The blocking task panicked or was cancelled by runtime shutdown.
            Err(e) => Err(error::invalid(format!("read: {e}"))),
        }
    }
```

### 3e. Image branch — pass the token, race the decode, guard the result

Current (read.rs:152-153):

```rust
        if let Some(mime) = crate::ops::ImageMime::from_file_head(&bytes) {
            return self.read_image(bytes, mime).await;
        }
```

Replacement:

```rust
        if let Some(mime) = crate::ops::ImageMime::from_file_head(&bytes) {
            let result = self.read_image(bytes, mime, &cancel).await?;
            // Pi's single common guard before `resolve` (read.ts:325) dominates BOTH image result
            // shapes — `ok` and `failed` — so a cancel landing during the decode/resize ladder
            // yields the abort, never an image block.
            if cancel.is_cancelled() {
                return Err(error::aborted());
            }
            return Ok(result);
        }
```

Current (read.rs:287-291):

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

Current (read.rs:304-311):

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
            // Pi `await`s `processImage` with the abort listener live (read.ts:257), so an abort
            // during it rejects at once. Here it is a SYNCHRONOUS decode + EXIF + resize +
            // JPEG-quality ladder invoked inline on the async worker: it observed no token AND
            // pinned a runtime thread for the whole ladder. Moving it onto the blocking pool is
            // what makes the race expressible at all. `Processed` is owned (`String` /
            // `Vec<String>`), so it crosses the boundary unchanged; `ImageMime` is `Copy`
            // (ops/mod.rs:52) and the two option fields are scalars, so nothing borrows `self`
            // across the spawn.
            let max_dim = self.opts.max_image_dim;
            let auto_resize = self.opts.auto_resize_images;
            let processing = tokio::task::spawn_blocking(move || {
                image_proc::process_image(&bytes, mime, max_dim, auto_resize)
            });
            let Some(joined) = cancel.run_until_cancelled(processing).await else {
                return Err(error::aborted());
            };
            let processed = joined.map_err(|e| error::invalid(format!("read: {e}")))?;
            match processed {
```

The `match` arms and both `Ok(ToolResult { … })` bodies inside them are unchanged; only the
scrutinee moves from the inline call to the joined binding, and `bytes` is now moved into the
blocking closure instead of borrowed.

Current (read.rs:357-361):

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
            // `resolve` (read.ts:325) still applies, and this keeps `cancel` used on BOTH cfg arms
            // without an `allow` attribute or an underscore-renamed parameter.
            if cancel.is_cancelled() {
                return Err(error::aborted());
            }
            // Image decoding is only compiled out under `--no-default-features`; the default build
            // always inlines. Surface the detected type + a build note (and the non-vision note).
            let mut note = format!(
```

### 3f. Guard both text-branch success returns

pi's read.ts:325 sits after the if/else chain that builds `outputText`, so it dominates the
first-line-exceeds shape as well as the truncated / user-limited / plain shapes.

Current (read.rs:213-220):

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
            // `firstLineExceedsLimit` branch assigns `outputText` and falls THROUGH to it.
            if cancel.is_cancelled() {
                return Err(error::aborted());
            }
            return Ok(ToolResult {
```

Current (read.rs:270-278):

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
        // Pi's `if (aborted) return;` immediately before `resolve` (read.ts:325). The `split('\n')`,
        // the window `join`, `truncate_head` and the continuation-notice formatting above are all
        // CPU work over a file that may be tens of megabytes, so this is a real window, not a
        // formality.
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

## 4. `crates/cyrup-tools/src/tools/grep.rs` — only if the sibling landed first

If `grep`'s task has already added `Cancelled` and `CancelReader` above its `MatchSink`
([grep.rs:233](../../../crates/cyrup-tools/src/tools/grep.rs)), delete those two definitions from
`grep.rs` and add:

```rust
use crate::ops::cancel_read::{CancelReader, Cancelled};
```

next to the existing `use crate::ops::{FsOps, WalkOpts};` (grep.rs:6). `struct Aborted`,
`MatchSink`'s `cancel` field, `search_one`'s threading and every call site stay exactly as that
brief specifies. If `read` lands first, `grep` writes that `use` line instead of the definitions.

## Files that change

| file | change |
| --- | --- |
| [crates/cyrup-tools/src/ops/cancel_read.rs](../../../crates/cyrup-tools/src/ops/) | **new** — `Cancelled` (+ `err`, `is`), `CancelReader<R>`, `read_to_end_cancellable`, `CHUNK` |
| [crates/cyrup-tools/src/ops/mod.rs](../../../crates/cyrup-tools/src/ops/mod.rs) | declare `pub(crate) mod cancel_read;` |
| [crates/cyrup-tools/src/tools/read.rs](../../../crates/cyrup-tools/src/tools/read.rs) | import the helpers; entry guard before `from_value`; guard after the variant-resolution loop; keep the guard before `access`→read; new `read_cancellable` replacing `self.fs.read(&abs)`; `read_image` takes `&CancelToken` and races `process_image` on the blocking pool; guards before the first-line-exceeds `Ok`, the text `Ok` and the image `Ok`; a `cancel` guard on the `not(inline-images)` arm |
| [crates/cyrup-tools/src/tools/grep.rs](../../../crates/cyrup-tools/src/tools/grep.rs) | one `use` line replacing the two locally-defined adapter types (see §4) |

No `Cargo.toml` change. No public API change — `lib.rs`'s `pub use ops::{…}` list is untouched.

## Concurrency rules that must be honoured

* **`spawn_blocking` is not abortable.** Dropping its `JoinHandle` detaches the thread; it does not
  stop it. Every outer race in this brief is paired with a token the blocking closure holds. Never
  ship one without the other.
* **`ErrorKind::Other`, never `Interrupted`.** `Interrupted` means "retry me" to `std`'s read loops
  and to `encoding_rs_io`'s BOM sniff; a sentinel of that kind spins forever.
* **The cancel path and the I/O-error path must never merge.** `Cancelled::is` is the only
  discriminator; a cancel becomes `error::aborted()`, everything else keeps `error::io`'s
  `"{path}: {io error}"`.
* **`is_cancelled()` cost.** An atomic load on an `Arc`'d node — once per 64 KiB and once per result
  return is not measurable against the I/O.
* **No new `Clone` derives, no `pub` on the adapter.** `pub(crate)` and no more.

## What remains genuinely uncertain

* **`read_stream` on an out-of-tree backend.** In-tree the seam switch is provably safe (§3d). An
  external `FsOps` that overrides `read_stream` with bytes that differ from its `read` would change
  what `read` returns; nothing in the trait contract forbids that, and `DistinctStreamFs`
  ([tests/isolation.rs:416-452](../../../crates/cyrup-tools/src/tests/isolation.rs)) exists to
  exploit exactly that freedom — though it is only ever driven through the `stream_text` helper,
  never through `ReadTool`.
* **Abort-latency floor on the local backend.** A 64 KiB `read(2)` on a stalled device (a hung NFS
  mount, a spun-down disk) is uninterruptible; the caller still returns at once via
  `run_until_cancelled`, but the detached blocking thread stays parked until that syscall returns.
  pi has the identical floor — libuv's read is not cancelled either.
* **`run_until_cancelled`'s completion bias.** Once polling has begun it is "biased towards the
  future completion" (tokio-util 0.7.18 `cancellation_token.rs:271-275`), so a cancel that lands in
  the same poll as the read finishing lets the read win. The inner token check then decides, and
  pi's promise is equally first-come — but do not expect a strict edge ordering here.
* **Guard placement inside the variant-resolution loop.** Left out deliberately for parity, but on a
  remote backend with several macOS filename variants that loop is several round-trips during which
  the token is unobserved. If that proves unacceptable it becomes a separate, declared
  `[CYRUP-DELTA]`, not a silent addition here.

## Definition of done

Behaviour that must hold, observable by running the agent:

1. `read` invoked with an already-cancelled token fails with `Operation aborted` and performs **no
   filesystem operation at all** — no `access`, no `read_stream` — even when the arguments are
   malformed: the abort wins over the deserialization error.
2. A token that fires while the file transfer is in flight makes `read` return `Operation aborted`
   without waiting for the transfer to finish — both for a slow `FsOps::read_stream` open (remote
   backend, whole transfer inside one `await`) and for a large local file mid-drain.
3. A mid-drain cancel also **stops the transfer**: the detached blocking task ceases reading within
   one 64 KiB buffer fill and does not accumulate the remainder of the file.
4. A genuine I/O failure during the drain still surfaces as `"{resolved path}: {io error}"` — the
   message `LocalFs::read` produced before this change — and is never reported as
   `Operation aborted`; conversely a cancellation is never reported as an I/O error.
5. A cancel that fires while an image is being decoded/resized makes `read` return
   `Operation aborted` instead of an image block, and the decode no longer occupies an async
   runtime worker thread while it runs.
6. No `Ok(ToolResult)` is returned by `read` after the token has fired, for **any** of the six
   result shapes: image-ok, image-failed, first-line-exceeds, truncated, user-limited, plain.
7. With a token that never fires, every result is byte-for-byte what it was before this change: the
   text window, the `[Showing lines a-b of N …]` and `[N more lines in file …]` notices, the
   `[Line N is X, exceeds Y limit …]` note, the truncation `details`, the `Read image file […]`
   note and its hints, the image block, `Offset … is beyond end of file (N lines total)`, and the
   raw errno text propagated from `access`.
8. Exactly one cancel-aware reader exists in `crates/cyrup-tools`, in `ops/cancel_read.rs`, and both
   `read` and `grep` use it. No second adapter exists under any other name, and it is not exported
   from the crate.
9. Nothing pi lacks is introduced: no new tool parameter, no new output text, no partial result on
   abort, no change to the `FsOps` trait or to any other tool. The only new externally visible
   outcome is `Operation aborted` arriving where the tool previously returned a successful result on
   a cancelled run.
