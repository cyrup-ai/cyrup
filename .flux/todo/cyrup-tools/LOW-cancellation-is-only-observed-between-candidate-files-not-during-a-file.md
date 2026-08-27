---
title: Cancellation is only observed between candidate files, not during a file's search
priority: LOW
tool: grep
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: aug
status: done
updated: 2026-08-27
---

# Cancellation is only observed between candidate files, not during a file's search

## Core objective

`grep` must observe the run's `CancelToken` **while a single candidate file is being searched**, not
only at the boundary between candidates — and a mid-file cancel must surface as
`error::aborted()` ("Operation aborted"), never as a normal partial result.

pi gets this for free because its search is an external ripgrep child it can kill; cyrup's search is
in-process inside `spawn_blocking`, and a `spawn_blocking` task **cannot be aborted from outside**.
The abort therefore has to be *pulled in* through the only two things the blocking searcher touches
that we own: the `io::Read` it pulls bytes from, and the `Sink` it pushes matches into. Both must
fail with a recognisable sentinel error the instant the token fires.

---

## What pi does — verified

[pi grep.ts](../../../tmp/pi/packages/coding-agent/src/core/tools/grep.ts) spawns real ripgrep
(`spawn(rgPath, args, …)`, grep.ts:226) and registers an abort listener that kills the child:

```ts
// grep.ts:240-250
const stopChild = (dueToLimit = false) => {
    if (!child.killed) {
        killedDueToLimit = dueToLimit;
        child.kill();
    }
};
const onAbort = () => {
    aborted = true;
    stopChild();
};
signal?.addEventListener("abort", onAbort, { once: true });
```

and on close rejects the whole promise:

```ts
// grep.ts:303-308
child.on("close", async (code) => {
    cleanup();
    if (aborted) {
        settle(() => reject(new Error("Operation aborted")));
        return;
    }
```

Every citation in the original audit (`:246-250`, `:240-245`, `:305-307`) checks out against
[grep.ts](../../../tmp/pi/packages/coding-agent/src/core/tools/grep.ts).

Note the shape of pi's `stopChild`: it is used for **two different reasons** — the match-limit stop
(`stopChild(true)`, grep.ts:292-295) which is a *successful* early exit, and the abort
(`stopChild()`, grep.ts:246-249) which is a *rejection*. That distinction is the one the Rust port
has to reproduce, and it rules out the shortcut suggested in the original write-up (see
"Correction" below).

## What cyrup-tools does today — verified

[grep.rs:73-83](../../../crates/cyrup-tools/src/tools/grep.rs) — `search_one` takes **no**
`CancelToken`:

```rust
    #[allow(clippy::too_many_arguments)]
    async fn search_one(
        &self,
        file: &std::path::Path,
        rel: &str,
        matcher: &grep_regex::RegexMatcher,
        context: usize,
        limit: usize,
        count: &mut usize,
        out: &mut Vec<String>,
        any_line_truncated: &mut bool,
    ) -> Result<(), ToolError> {
```

[grep.rs:119-137](../../../crates/cyrup-tools/src/tools/grep.rs) — the blocking search runs to
completion and *discards* the searcher's result entirely:

```rust
        let matches: Vec<(u64, Vec<u8>)> = tokio::task::spawn_blocking(move || {
            let mut searcher: Searcher = SearcherBuilder::new()
                .line_number(true)
                .binary_detection(BinaryDetection::quit(b'\x00'))
                .build();
            let mut matches: Vec<(u64, Vec<u8>)> = Vec::new();
            let mut local = 0usize;
            {
                let sink = MatchSink {
                    matches: &mut matches,
                    count: &mut local,
                    limit: remaining,
                };
                let _ = searcher.search_reader(&matcher_owned, reader, sink);
            }
            matches
        })
        .await
        .map_err(|e| error::invalid(format!("grep: {e}")))?;
```

[grep.rs:388-396](../../../crates/cyrup-tools/src/tools/grep.rs) — the limitation, acknowledged in
the source:

```rust
                // The `select!` below only observes a cancel while it is parked on `walk.next()`;
                // one that lands while `search_one` is running is observed here, on the next turn
                // — the same per-candidate granularity the staged loop had.
                if cancel.is_cancelled() {
                    return Err(error::aborted());
                }
                tokio::select! {
                    _ = cancel.cancelled() => return Err(error::aborted()),
```

Confirmed absent elsewhere: **no `impl io::Read` exists anywhere in `crates/`** (`impl io::Read` /
`impl std::io::Read` / `impl Read for` all return zero hits), so no cancel-aware reader can be
reused. [`LocalFs::read_stream`](../../../crates/cyrup-tools/src/ops/local/fs.rs) returns a bare
`Box::new(std::fs::File)` (fs.rs:73-80) and the
[`FsOps::read_stream` default](../../../crates/cyrup-tools/src/ops/mod.rs) a plain
`Cursor` (ops/mod.rs:341-343). [`MatchSink`](../../../crates/cyrup-tools/src/tools/grep.rs)
(grep.rs:243-247) holds no token.

No outer race hides the gap either:
[exec.rs](../../../crates/cyrup-agent/src/agent/run/tools/exec.rs) awaits `tool.execute` to
completion in both the parallel (exec.rs:128-136) and the sequential (exec.rs:260-290) paths — the
future is never dropped, so in-tool cancellation is the *only* lever.

## User-visible impact

Pressing Esc while grep is scanning one very large file does nothing until that file finishes. Worse
on the single-file branch ([grep.rs:342-360](../../../crates/cyrup-tools/src/tools/grep.rs)): a
mid-scan abort there yields a **normal successful result**, where pi rejects with
`Operation aborted`.

---

## Correction to the original parity action

The original note proposed *"have the `Sink` return `Ok(false)` once the token is cancelled"*.
**That is wrong and must not be implemented.** `Ok(false)` is `grep_searcher`'s ordinary
"I have enough, stop" signal — it is exactly what the existing match-limit line
`Ok(*self.count < self.limit)` (grep.rs:258) already uses, and it ends `search_reader`
**successfully**. The caller cannot tell the two apart, so a cancel would produce a normal partial
grep result instead of pi's rejection. The abort must leave the searcher as an **`Err`** carrying a
recognisable payload.

## Research: how the error actually travels (and the trap in it)

1. `LineBuffer::fill` (grep-searcher 0.1.16, `src/line_buffer.rs:406-418`) calls
   `rdr.read(...)?` once per buffer refill and **propagates the error without retrying**.
   `DEFAULT_BUFFER_CAPACITY` is `64 * (1 << 10)` = 64 KiB (`line_buffer.rs:6`), so one reader check
   per 64 KiB chunk is the achievable granularity.
2. `ReadByLine::fill` (`src/searcher/glue.rs:58-66`) converts it with
   `S::Error::error_io(err)`, and `impl SinkError for io::Error` is
   `fn error_io(err: io::Error) -> io::Error { err }` (`src/sink.rs:47-49`) — the error, **including
   its boxed payload**, comes back out of `search_reader` verbatim.
3. **The trap:** `Searcher::search_reader` wraps the caller's reader in `encoding_rs_io`'s
   `DecodeReaderBytes` for BOM sniffing (`src/searcher/mod.rs`, `build_with_buffer`). Its
   `detect()` → `peek_bom()` → `util::read_full` helper contains:

   ```rust
   Err(ref e) if e.kind() == io::ErrorKind::Interrupted => {}
   ```

   inside a `while !buf.is_empty()` loop (`encoding_rs_io-0.1.7/src/util.rs:230-241`). A sentinel of
   kind `ErrorKind::Interrupted` would therefore **spin forever on the very first read of every
   file**. The sentinel MUST use `ErrorKind::Other` (`io::Error::other`). Every other hop
   (`DecodeReaderBytes::read`/`fill`/`transcode`, `LineBuffer::fill`) uses plain `?` with no kind
   inspection, so `Other` passes straight through.
4. `CancellationToken::run_until_cancelled` (tokio-util 0.7.18,
   `sync/cancellation_token.rs:280-293`) short-circuits to `None` when the token is *already*
   cancelled, so it doubles as an entry guard for the two `await` points in `search_one`.

---

## Required implementation

### File 1 — `crates/cyrup-tools/src/tools/grep.rs` (the only source file that changes)

#### 1a. Add the sentinel, the reader, and the blocking-outcome marker

Insert these items immediately **above** the `MatchSink` doc comment (currently at
[grep.rs:233](../../../crates/cyrup-tools/src/tools/grep.rs)):

```rust
/// The payload attached to the `io::Error` that a cancelled [`CancelReader`] read — or a cancelled
/// [`MatchSink`] callback — returns, so [`GrepTool::search_one`] can tell "the token fired
/// mid-file" apart from a genuine read failure. That distinction is load-bearing: a read failure
/// must stay a SILENT SKIP (rg emits no match events for a file it cannot read), while a cancel
/// must become [`error::aborted`].
///
/// The kind is deliberately [`std::io::ErrorKind::Other`] and NOT `Interrupted`. `search_reader`
/// wraps the reader in `encoding_rs_io`'s `DecodeReaderBytes`, whose BOM sniff goes through
/// `util::read_full`, and that helper RETRIES on `ErrorKind::Interrupted`
/// (`Err(ref e) if e.kind() == io::ErrorKind::Interrupted => {}` inside its `while !buf.is_empty()`
/// loop, encoding_rs_io-0.1.7 `src/util.rs:230-241`). An `Interrupted` sentinel would spin forever
/// on the first read of every candidate instead of aborting it.
#[derive(Debug)]
struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Same wording as `error::aborted`, so a stray leak reads identically to Pi's rejection.
        f.write_str("Operation aborted")
    }
}

impl std::error::Error for Cancelled {}

impl Cancelled {
    /// The `io::Error` carrying this marker. `grep_searcher`'s `SinkError for io::Error` forwards
    /// reader errors verbatim (`fn error_io(err: io::Error) -> io::Error { err }`,
    /// grep-searcher-0.1.16 `src/sink.rs:47-49`), so the boxed payload survives the trip out of
    /// `search_reader` unchanged.
    fn err() -> std::io::Error {
        std::io::Error::other(Cancelled)
    }

    /// Recover the marker from whatever `search_reader` returned.
    fn is(e: &std::io::Error) -> bool {
        e.get_ref().is_some_and(|src| src.is::<Cancelled>())
    }
}

/// The blocking search stopped because the token fired, not because the file ended.
struct Aborted;

/// A [`std::io::Read`] adapter that turns the run's [`CancelToken`] into an error the moment it
/// fires, so a cancel is observed DURING one candidate's search rather than at the next file
/// boundary.
///
/// This is the in-process stand-in for Pi's `onAbort` → `stopChild()` listener (grep.ts:240-250):
/// Pi kills the ripgrep child, which ends the search at once. cyrup has no child to kill, and a
/// `tokio::task::spawn_blocking` task cannot be aborted from outside, so the abort has to be PULLED
/// IN by the one thing the blocking `search_reader` call keeps asking us for — bytes.
///
/// Granularity: `grep_searcher`'s `LineBuffer::fill` calls `rdr.read` once per buffer refill and
/// `?`-propagates the error without retrying (grep-searcher-0.1.16 `src/line_buffer.rs:406-418`),
/// and `DEFAULT_BUFFER_CAPACITY` is 64 KiB (`src/line_buffer.rs:6`). So the worst case after a
/// cancel is one 64 KiB chunk plus the regex scan over it, instead of one whole file.
struct CancelReader<R> {
    inner: R,
    cancel: CancelToken,
}

impl<R> CancelReader<R> {
    fn new(inner: R, cancel: CancelToken) -> Self {
        Self { inner, cancel }
    }
}

impl<R: std::io::Read> std::io::Read for CancelReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        // Before: never start another chunk once the token has fired.
        if self.cancel.is_cancelled() {
            return Err(Cancelled::err());
        }
        let n = self.inner.read(buf)?;
        // After: a cancel that landed WHILE this read was parked — a slow disk, a network-backed
        // `FsOps`, a `read_stream` that is not a plain `File` — is observed now rather than one
        // chunk later. Dropping the bytes just read is sound because the search is being abandoned:
        // `search_one` returns `error::aborted()` and no partial result is ever emitted.
        if self.cancel.is_cancelled() {
            return Err(Cancelled::err());
        }
        Ok(n)
    }
}
```

No new `use` lines are needed: `CancelToken` is already imported at
[grep.rs:10](../../../crates/cyrup-tools/src/tools/grep.rs), and `std::io` / `std::fmt` /
`std::error` are spelled out in full above to match the file's existing style
(`std::io::Error` at grep.rs:250, `std::path::Path` at grep.rs:75).

#### 1b. Give `MatchSink` the token — the second abort hook

Current ([grep.rs:243-259](../../../crates/cyrup-tools/src/tools/grep.rs)):

```rust
struct MatchSink<'a> {
    matches: &'a mut Vec<(u64, Vec<u8>)>,
    count: &'a mut usize,
    limit: usize,
}

impl Sink for MatchSink<'_> {
    type Error = std::io::Error;

    fn matched(&mut self, _s: &Searcher, m: &SinkMatch<'_>) -> Result<bool, std::io::Error> {
        // `SinkMatch::bytes` is the matched line INCLUDING its terminator, which is exactly what
        // ripgrep serialises into `data.lines.text` — hence Pi's `.replace(/\n$/,"")` below.
        self.matches
            .push((m.line_number().unwrap_or(0), m.bytes().to_vec()));
        *self.count += 1;
        Ok(*self.count < self.limit)
    }
}
```

Replacement:

```rust
struct MatchSink<'a> {
    matches: &'a mut Vec<(u64, Vec<u8>)>,
    count: &'a mut usize,
    limit: usize,
    /// The SECOND abort hook, covering the match-DENSE case [`CancelReader`] cannot: one 64 KiB
    /// refill can fire `matched` thousands of times, and every one of those callbacks runs before
    /// the searcher asks the reader for another byte. Owned rather than borrowed — the token is an
    /// `Arc` handle, so the clone is a refcount bump and it keeps the `'a` lifetime tied to the
    /// caller's buffers alone.
    cancel: CancelToken,
}

impl Sink for MatchSink<'_> {
    type Error = std::io::Error;

    fn matched(&mut self, _s: &Searcher, m: &SinkMatch<'_>) -> Result<bool, std::io::Error> {
        // NOT `Ok(false)`. That is `search_reader`'s ordinary "stop, you have enough" signal — the
        // very thing the limit line below uses — and it ends the search SUCCESSFULLY, so the caller
        // could not distinguish a cancel from a satisfied match budget and would emit a normal
        // partial result. Pi rejects with `Operation aborted` instead (grep.ts:305-307), so the
        // cancel has to leave as an `Err` the caller can recognise.
        if self.cancel.is_cancelled() {
            return Err(Cancelled::err());
        }
        // `SinkMatch::bytes` is the matched line INCLUDING its terminator, which is exactly what
        // ripgrep serialises into `data.lines.text` — hence Pi's `.replace(/\n$/,"")` below.
        self.matches
            .push((m.line_number().unwrap_or(0), m.bytes().to_vec()));
        *self.count += 1;
        Ok(*self.count < self.limit)
    }
}
```

#### 1c. `search_one` signature — thread the token

Current ([grep.rs:72-83](../../../crates/cyrup-tools/src/tools/grep.rs)):

```rust
    #[allow(clippy::too_many_arguments)]
    async fn search_one(
        &self,
        file: &std::path::Path,
        rel: &str,
        matcher: &grep_regex::RegexMatcher,
        context: usize,
        limit: usize,
        count: &mut usize,
        out: &mut Vec<String>,
        any_line_truncated: &mut bool,
    ) -> Result<(), ToolError> {
```

Replacement — one new parameter, placed immediately after `matcher` (the `#[allow]` already covers
the arity):

```rust
    #[allow(clippy::too_many_arguments)]
    async fn search_one(
        &self,
        file: &std::path::Path,
        rel: &str,
        matcher: &grep_regex::RegexMatcher,
        // Observed at BOTH `await` points below and, via `CancelReader` and `MatchSink`, inside
        // the blocking search itself. `Err(error::aborted())` here propagates through the `?` at
        // each call site and out of `execute`, matching Pi's `Operation aborted` rejection.
        cancel: &CancelToken,
        context: usize,
        limit: usize,
        count: &mut usize,
        out: &mut Vec<String>,
        any_line_truncated: &mut bool,
    ) -> Result<(), ToolError> {
```

#### 1d. Race the open

Current ([grep.rs:93-95](../../../crates/cyrup-tools/src/tools/grep.rs)):

```rust
        let Ok(reader) = self.fs.read_stream(file).await else {
            return Ok(());
        };
```

Replacement:

```rust
        // A cancel landing while the open is in flight must not wait for the open to finish — a
        // remote/RPC `FsOps` can park here for a long time. `run_until_cancelled` returns `None`
        // immediately when the token is ALREADY cancelled (tokio-util 0.7.18
        // `sync/cancellation_token.rs:280-293`), so this doubles as the entry guard, and otherwise
        // drops the open future the moment the token fires.
        //
        // `None` is an abort; a `Some(Err(_))` is still the pre-existing skip: rg simply emits no
        // match events for a file it cannot open.
        let Some(opened) = cancel.run_until_cancelled(self.fs.read_stream(file)).await else {
            return Err(error::aborted());
        };
        let Ok(reader) = opened else {
            return Ok(());
        };
```

#### 1e. Wrap the reader, arm the sink, and classify the outcome

Current ([grep.rs:113-137](../../../crates/cyrup-tools/src/tools/grep.rs)):

```rust
        let matcher_owned = matcher.clone();
        // `MatchSink` counts against the REMAINING budget; the caller's global `count` is advanced
        // by however many this file contributed. Pi's cap is global too — its line handler ignores
        // every event once `matchCount >= effectiveLimit` (grep.ts:278) — so a file can only ever
        // fill the gap.
        let remaining = limit.saturating_sub(*count);
        let matches: Vec<(u64, Vec<u8>)> = tokio::task::spawn_blocking(move || {
            let mut searcher: Searcher = SearcherBuilder::new()
                .line_number(true)
                .binary_detection(BinaryDetection::quit(b'\x00'))
                .build();
            let mut matches: Vec<(u64, Vec<u8>)> = Vec::new();
            let mut local = 0usize;
            {
                let sink = MatchSink {
                    matches: &mut matches,
                    count: &mut local,
                    limit: remaining,
                };
                let _ = searcher.search_reader(&matcher_owned, reader, sink);
            }
            matches
        })
        .await
        .map_err(|e| error::invalid(format!("grep: {e}")))?;
```

Replacement:

```rust
        let matcher_owned = matcher.clone();
        // The token is moved INTO the blocking task rather than polled from outside it: a
        // `spawn_blocking` task owns an OS thread and cannot be aborted by dropping its
        // `JoinHandle`, so the only way out of `search_reader` is for the work itself to fail.
        // Cloning is an `Arc` refcount bump (`CancelToken` is `tokio_util::sync::CancellationToken`,
        // cyrup-core `cancel.rs:9`).
        let cancel_task = cancel.clone();
        // `MatchSink` counts against the REMAINING budget; the caller's global `count` is advanced
        // by however many this file contributed. Pi's cap is global too — its line handler ignores
        // every event once `matchCount >= effectiveLimit` (grep.ts:278) — so a file can only ever
        // fill the gap.
        let remaining = limit.saturating_sub(*count);
        let searched: Result<Vec<(u64, Vec<u8>)>, Aborted> = tokio::task::spawn_blocking(move || {
            let mut searcher: Searcher = SearcherBuilder::new()
                .line_number(true)
                .binary_detection(BinaryDetection::quit(b'\x00'))
                .build();
            let mut matches: Vec<(u64, Vec<u8>)> = Vec::new();
            let mut local = 0usize;
            let outcome = {
                let sink = MatchSink {
                    matches: &mut matches,
                    count: &mut local,
                    limit: remaining,
                    cancel: cancel_task.clone(),
                };
                searcher.search_reader(
                    &matcher_owned,
                    CancelReader::new(reader, cancel_task),
                    sink,
                )
            };
            match outcome {
                // The searcher's error is no longer thrown away wholesale: a cancel marker is an
                // abort, and EVERY other `io::Error` keeps the previous `let _ = …` semantics —
                // whatever was collected before the failure stands and the walk moves on, because
                // rg emits no match events for a file it cannot read.
                Err(e) if Cancelled::is(&e) => Err(Aborted),
                _ => Ok(matches),
            }
        })
        .await
        .map_err(|e| error::invalid(format!("grep: {e}")))?;

        let Ok(matches) = searched else {
            return Err(error::aborted());
        };
```

#### 1f. Race the context re-read

Current ([grep.rs:162-163](../../../crates/cyrup-tools/src/tools/grep.rs)):

```rust
        let src_lines: Option<Vec<String>> = if matches.iter().any(|(_, r)| takes_block(r)) {
            match self.fs.read(file).await {
```

Replacement:

```rust
        let src_lines: Option<Vec<String>> = if matches.iter().any(|(_, r)| takes_block(r)) {
            // This is a WHOLE-FILE read of a file that already matched — on a multi-hundred-MB
            // candidate it is the second place a cancel could be stranded, so it is raced too.
            // `Err(_)` stays Pi's `catch { lines = [] }` (grep.ts:212-214); only a `None` — the
            // token firing — aborts.
            let Some(read) = cancel.run_until_cancelled(self.fs.read(file)).await else {
                return Err(error::aborted());
            };
            match read {
```

The rest of that `match` (the `Ok(b) => { … }` / `Err(_) => None` arms and the closing braces at
[grep.rs:164-177](../../../crates/cyrup-tools/src/tools/grep.rs)) is unchanged.

#### 1g. Call site 1 — the single-file branch

Current ([grep.rs:350-360](../../../crates/cyrup-tools/src/tools/grep.rs)):

```rust
            self.search_one(
                &search_root,
                &rel,
                &matcher,
                context,
                limit,
                &mut count,
                &mut out,
                &mut any_line_truncated,
            )
            .await?;
```

Replacement:

```rust
            self.search_one(
                &search_root,
                &rel,
                &matcher,
                &cancel,
                context,
                limit,
                &mut count,
                &mut out,
                &mut any_line_truncated,
            )
            .await?;
```

This is the branch that closes the sharpest divergence: a mid-scan abort on `path`-points-at-a-file
now returns `Operation aborted` instead of a normal successful result.

#### 1h. Call site 2 — the fused walk loop

Current ([grep.rs:415-425](../../../crates/cyrup-tools/src/tools/grep.rs)):

```rust
                                self.search_one(
                                    &w.path,
                                    &rel,
                                    &matcher,
                                    context,
                                    limit,
                                    &mut count,
                                    &mut out,
                                    &mut any_line_truncated,
                                )
                                .await?;
```

Replacement:

```rust
                                self.search_one(
                                    &w.path,
                                    &rel,
                                    &matcher,
                                    &cancel,
                                    context,
                                    limit,
                                    &mut count,
                                    &mut out,
                                    &mut any_line_truncated,
                                )
                                .await?;
```

#### 1i. Retire the stale comment

Current ([grep.rs:388-393](../../../crates/cyrup-tools/src/tools/grep.rs)) documents the limitation
this task removes and is now false:

```rust
                // The `select!` below only observes a cancel while it is parked on `walk.next()`;
                // one that lands while `search_one` is running is observed here, on the next turn
                // — the same per-candidate granularity the staged loop had.
                if cancel.is_cancelled() {
                    return Err(error::aborted());
                }
```

Replacement — the check itself STAYS (it is the cheap fast path that avoids even opening the next
candidate); only the prose changes:

```rust
                // The `select!` below observes a cancel while parked on `walk.next()`; one that
                // lands mid-candidate is observed INSIDE `search_one`, which threads the token
                // through `CancelReader` and `MatchSink` into the blocking searcher. This check is
                // the cheap fast path: it skips opening the next candidate at all.
                if cancel.is_cancelled() {
                    return Err(error::aborted());
                }
```

The `tokio::select!` at [grep.rs:394-396](../../../crates/cyrup-tools/src/tools/grep.rs) is
unchanged — it still covers a cancel that lands while the walk is parked between candidates.

### Files that do NOT change

- [ops/mod.rs](../../../crates/cyrup-tools/src/ops/mod.rs) — `FsOps::read_stream`'s
  `Box<dyn std::io::Read + Send>` return type is already exactly what `CancelReader<R>` wraps
  (`CancelReader<Box<dyn io::Read + Send>>` is itself `io::Read`, and `Send` because both fields
  are). The trait, its doc contract, and its default body stay as they are.
- [ops/local/fs.rs](../../../crates/cyrup-tools/src/ops/local/fs.rs) — `LocalFs::read_stream` keeps
  returning a plain `std::fs::File`; the cancellation is layered on at the consumer, not baked into
  the seam, so every `FsOps` backend and every `isolation/` decorator gets it for free.
- [error.rs](../../../crates/cyrup-tools/src/error.rs) — `error::aborted()` (error.rs:117-119)
  already produces `"Operation aborted"`; no new helper.
- [cancel.rs](../../../crates/cyrup-core/src/cancel.rs) — no change; `CancelToken` is just
  `tokio_util::sync::CancellationToken` (cancel.rs:9) and already exposes `is_cancelled`,
  `cancelled` and `run_until_cancelled`.
- [find.rs](../../../crates/cyrup-tools/src/tools/find.rs) — out of scope. `find` never searches
  file CONTENT, so it has no in-file blocking window; its per-entry checks (find.rs:116, :174, :184)
  are already at the right granularity.

---

## Concurrency notes that must be honoured

- **`spawn_blocking` is not abortable.** Dropping its `JoinHandle` does not stop the thread. Every
  design that tries to cancel from outside (a `select!` around the `.await`, `abort()`, a timeout)
  leaks a live thread still burning CPU on the file. The token must be *inside* the closure. That is
  why `CancelReader` and the `MatchSink` field both exist rather than an outer race.
- **Two hooks, not one.** `CancelReader` covers a scan-heavy file (few matches, many bytes);
  `MatchSink::matched` covers a match-dense file (one 64 KiB refill producing thousands of
  callbacks). Either alone leaves a real stall window.
- **`is_cancelled()` cost.** It is an atomic load on an `Arc`'d node. Once per 64 KiB read and once
  per matched line is not measurable against the regex scan.
- **Do not `#[derive(Clone)]` on `CancelReader`** and do not make it public. It is a private
  adapter local to `grep.rs`; widening it is redesign, not parity.
- **`Cancelled` must not use `ErrorKind::Interrupted`** — see the trap documented above. Use
  `std::io::Error::other`, which is `ErrorKind::Other` plus a boxed payload (stable since 1.74;
  the workspace pins `rust-version = "1.96"`, edition 2024).
- **Do not change the discard semantics for non-cancel errors.** The `_ => Ok(matches)` arm is
  deliberate: a permission error, a mid-read I/O error, or a decode failure must keep skipping the
  file silently, because ripgrep emits no match events for a file it cannot read.

## What remains genuinely uncertain

1. **Sub-chunk latency on a pathological line.** `LineBuffer::fill` loops internally until it finds
   a line terminator, growing the buffer, before it returns. A file that is one multi-megabyte line
   with no `\n` will do several `read` calls inside that loop — all of which *are* cancel-observing —
   but the regex scan over the final assembled buffer is not interruptible. This is bounded by the
   searcher's own heap limit and is strictly better than today's whole-file window; it is not worth
   further machinery.
2. **`BinaryDetection::quit` interaction.** When a NUL is hit, `LineBuffer::fill` returns early
   without calling `read` again (`line_buffer.rs:410-412`), so the reader hook goes quiet for the
   tail of a binary file. In practice `quit` ends the file almost immediately, so the window is tiny.
   Confirm by observation rather than assuming.
3. **The `matches.is_empty()` early return.** After the change, a cancel that fires *between* the
   blocking search completing and the `matches.is_empty()` check
   ([grep.rs:139-141](../../../crates/cyrup-tools/src/tools/grep.rs)) is still only observed at the
   next loop turn. Leaving that alone is correct — it is a nanosecond-scale window with no blocking
   work in it — but do not be surprised by it.

---

## Definition of done

Stated as behaviour that must hold, observable by running the agent:

1. Cancelling the run while `grep` is scanning a **single very large file** (the
   `path`-points-at-a-file branch) causes `execute` to return `Err` with the message
   `Operation aborted`, within roughly the time it takes to read and scan one 64 KiB chunk —
   not after the whole file has been searched, and never as a successful `ToolResult`.
2. Cancelling the run while `grep` is inside `search_one` on a **directory walk** aborts during that
   candidate, not at the next candidate boundary, and `execute` returns `Operation aborted`.
3. Cancelling while `grep` is scanning a file that produces a very high match rate aborts at the
   next matched line, without waiting for the searcher to ask the reader for another chunk.
4. Cancelling while `grep` is blocked on `FsOps::read_stream` (the open) or on the `context > 0`
   whole-file `FsOps::read` returns `Operation aborted` immediately rather than after that
   operation completes.
5. No `spawn_blocking` thread survives the abort still searching: the blocking task ends because
   `search_reader` returned the cancel error, not because the caller stopped awaiting it.
6. With **no** cancellation, output is byte-for-byte identical to today's for every input — the same
   rows, the same ordering, the same `[… limit reached …]` notice bracket, the same
   `No matches found`. In particular the match-limit stop still ends the search *successfully* via
   `Ok(false)` and still reports `N matches limit reached`, never `Operation aborted`.
7. A file that cannot be opened, or that fails mid-read for a non-cancel reason, is still skipped
   silently and the walk continues — unchanged from today.
8. Binary files are still cut short by `BinaryDetection::quit(b'\x00')` and contribute no rows.
9. No behaviour pi does not have is introduced: no new tool parameter, no new output text, no
   partial-result-on-abort, no change to `FsOps` or to any other tool.
