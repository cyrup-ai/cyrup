---
title: A UTF-8 BOM in command output is stripped by pi but retained by cyrup
priority: LOW
tool: bash
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: exec
status: in-progress
updated: 2026-08-27
---

# A UTF-8 BOM in command output is stripped by pi but retained by cyrup

## Core objective

The `bash` tool's **model-visible** output must not begin with a `U+FEFF` byte-order mark, and the
BOM must not be counted in the reported `totalBytes` / line-length numbers — exactly as pi's single
default `TextDecoder` already guarantees. The BOM must still be counted as **raw** bytes and must
still reach the spilled full-output temp file verbatim, because pi keeps it there too.

Concretely: `OutputAccumulator` grows a stream-head BOM filter that sits between the raw chunk and
the two consumers that model pi's *decoded* text — the decode counters and the rolling preview tail
— while the raw byte counter and the temp-file spill keep seeing the untouched chunk.

---

## Verified pi behaviour

[pi output-accumulator.ts:40](../../../tmp/pi/packages/coding-agent/src/core/tools/output-accumulator.ts)
declares **one** decoder per accumulator:

```ts
private readonly decoder = new TextDecoder();
```

`ignoreBOM` defaults to `false`, so the WHATWG decoder removes a leading `EF BB BF` from the **start
of the stream** (and only there — a BOM appearing later, or a second BOM immediately after the
first, is emitted as a real `U+FEFF`). The decoder is stateful across chunks, so a BOM split across
two `append` calls is still removed.

[pi output-accumulator.ts:64-78](../../../tmp/pi/packages/coding-agent/src/core/tools/output-accumulator.ts)
shows the exact split of responsibilities this task must reproduce:

```ts
append(data: Buffer): void {
	if (this.finished) { throw new Error("Cannot append to a finished output accumulator"); }

	this.totalRawBytes += data.length;                                   // :69  RAW — BOM counted
	this.appendDecodedText(this.decoder.decode(data, { stream: true })); // :70  DECODED — BOM gone

	if (this.tempFileStream || this.shouldUseTempFile()) {
		this.ensureTempFile();
		this.tempFileStream?.write(data);                                // :74  RAW — BOM written
	} else if (data.length > 0) {
		this.rawChunks.push(data);                                       // :76  RAW — BOM buffered
	}
}
```

So in pi the BOM:

| consumer | pi line | sees the BOM? |
|---|---|---|
| `totalRawBytes` (gates `shouldUseTempFile`) | [:69](../../../tmp/pi/packages/coding-agent/src/core/tools/output-accumulator.ts), [:205-208](../../../tmp/pi/packages/coding-agent/src/core/tools/output-accumulator.ts) | **yes** |
| temp file / `rawChunks` replay | [:74-77](../../../tmp/pi/packages/coding-agent/src/core/tools/output-accumulator.ts), [:217-219](../../../tmp/pi/packages/coding-agent/src/core/tools/output-accumulator.ts) | **yes** |
| `totalDecodedBytes` (`truncation.totalBytes`) | [:153-154](../../../tmp/pi/packages/coding-agent/src/core/tools/output-accumulator.ts), [:105](../../../tmp/pi/packages/coding-agent/src/core/tools/output-accumulator.ts) | no |
| `tailText` → `getSnapshotText` → `snapshot.content` | [:155](../../../tmp/pi/packages/coding-agent/src/core/tools/output-accumulator.ts), [:196-203](../../../tmp/pi/packages/coding-agent/src/core/tools/output-accumulator.ts) | no |
| `totalLines` / `currentLineBytes` | [:161-176](../../../tmp/pi/packages/coding-agent/src/core/tools/output-accumulator.ts) | no |

The final flush is `this.decoder.decode()` with no `stream` flag at
[pi output-accumulator.ts:85](../../../tmp/pi/packages/coding-agent/src/core/tools/output-accumulator.ts),
which turns any still-incomplete trailing sequence into one `U+FFFD`. A stream that ends **inside**
a BOM prefix (`EF`, or `EF BB`, and nothing else) therefore decodes to exactly one `U+FFFD`,
3 decoded bytes — it was never a BOM.

The raw `Buffer` is handed to `append` untouched by
[pi bash.ts:410-413](../../../tmp/pi/packages/coding-agent/src/core/tools/bash.ts):

```ts
const handleData = (data: Buffer) => {
	if (!acceptingOutput) return;
	output.append(data);
	scheduleOutputUpdate();
};
```

`handleData` is the single sink for **both** stdout and stderr, so "start of stream" means the first
bytes emitted on *either* fd, not the first bytes of stdout.

---

## Verified cyrup behaviour (the gap)

[output.rs:156-181](../../../crates/cyrup-tools/src/output.rs) `append` forwards the **raw** chunk to
every consumer:

```rust
pub fn append(&mut self, chunk: &[u8]) {
    if chunk.is_empty() {
        return;
    }
    self.total_raw_bytes += chunk.len();
    // Decode through the streaming UTF-8 decoder so totals/line-counts/last-line bytes reflect
    // the DECODED text (Pi parity, UM-8). For valid UTF-8 this equals the raw counts.
    self.decode_into_counters(chunk);

    // Rolling tail for the preview.
    self.buf.extend_from_slice(chunk);
    if self.buf.len() > self.cap {
        let start = self.buf.len() - self.cap;
        self.buf.drain(..start);
    }
    ...
}
```

- [output.rs:76-107](../../../crates/cyrup-tools/src/output.rs) `decode_into_counters` is a
  hand-rolled streaming UTF-8 decoder built on `std::str::from_utf8` with a `pending` carry. It has
  **no BOM branch**, so `U+FEFF` flows into
  [`append_decoded_text`](../../../crates/cyrup-tools/src/output.rs) (:119-137) and adds 3 to
  `total_decoded_bytes` and to `current_line_bytes`.
- [output.rs:206-208](../../../crates/cyrup-tools/src/output.rs) `tail_string` is
  `String::from_utf8_lossy(&self.buf)` over the raw rolling buffer, so the BOM is at the head of the
  preview text.
- Both consumers feed it straight to `truncate_tail`, which does no character filtering:
  [bash.rs:392-394](../../../crates/cyrup-tools/src/tools/bash.rs) (final result) and
  [bash.rs:573-574](../../../crates/cyrup-tools/src/tools/bash.rs) (mid-stream `onUpdate`).
- Both cyrup stdout and stderr reads call the same `on_data` closure —
  [proc.rs:199](../../../crates/cyrup-tools/src/ops/local/proc.rs) and
  [proc.rs:216](../../../crates/cyrup-tools/src/ops/local/proc.rs) — funnelled through one unbounded
  channel into one `OutputAccumulator`
  ([bash.rs:262](../../../crates/cyrup-tools/src/tools/bash.rs),
  [bash.rs:326-346](../../../crates/cyrup-tools/src/tools/bash.rs)). The "one stream head" model is
  therefore already correct; only the filter is missing.

`OutputAccumulator` has exactly two users, both in
[bash.rs](../../../crates/cyrup-tools/src/tools/bash.rs) (lines 262, 393, 573). Nothing else in the
workspace constructs one, so the fix is fully contained in `output.rs`.

### Citation corrections against the current tree

- `output.rs:205-208` in the original write-up: `tail_string` is `:206-208`, doc comment at `:205`.
  Everything else in the finding (`output.rs:76-107`, `bash.rs:393-394`, `bash.rs:573`,
  `edit_diff.rs:26-31`, `ext_config.rs:529`, `cyrup-mcp/ui.rs:403`, `bash.ts:410-412`,
  `output-accumulator.ts:40`) is accurate.
- The adversary note cites `totalRawBytes (output-accumulator.ts:114)`. Line 114 is the `snapshot()`
  return object, which does **not** carry `totalRawBytes` at all. The correct citations are `:46`
  (field), `:69` (increment) and `:205-208` (`shouldUseTempFile`). The substantive claim — pi's raw
  counter still counts the BOM and pi's spill file still contains it — is correct and is preserved
  as required behaviour below.

### Why `edit_diff::strip_bom` is not reused here

[edit_diff.rs:26-31](../../../crates/cyrup-tools/src/tools/edit_diff.rs):

```rust
/// Strip a leading UTF-8 BOM, returning `(had_bom, body)`.
pub fn strip_bom(s: &str) -> (bool, &str) {
    match s.strip_prefix(BOM) {
        Some(rest) => (true, rest),
        None => (false, s),
    }
}
```

That helper takes a **complete `&str`** already read from disk, and its whole purpose in
[edit.rs:249,267-271](../../../crates/cyrup-tools/src/tools/edit.rs) is to *re-attach* the BOM after
editing. The accumulator needs the opposite contract at a different layer: a **byte-level**,
**chunk-straddling**, **stream-head-only, one-shot** filter that runs before UTF-8 validation, so a
BOM split as `EF` / `BB BF` across two reads is still removed and a lone `EF` at end-of-stream still
decodes to `U+FFFD`. `strip_bom` cannot express any of that, so the filter lives next to the decoder
state it belongs to and is *not* a duplicate of it.

---

## Required implementation

### The only file that changes: [`crates/cyrup-tools/src/output.rs`](../../../crates/cyrup-tools/src/output.rs)

No change to `crates/cyrup-core` (`Truncation` / `BashDetails` live in
[truncate.rs](../../../crates/cyrup-tools/src/truncate.rs) and
[details.rs](../../../crates/cyrup-tools/src/details.rs), and neither data shape moves).
No change to [bash.rs](../../../crates/cyrup-tools/src/tools/bash.rs) — every call site keeps its
current signature; the behaviour change is entirely inside the accumulator.

#### 1. New import and constant

Current header:

```rust
use crate::ops::local::unique_suffix;
use std::io::Write;
use std::path::PathBuf;
```

Replacement:

```rust
use crate::ops::local::unique_suffix;
use std::borrow::Cow;
use std::io::Write;
use std::path::PathBuf;

/// `U+FEFF` encoded as UTF-8 — the byte-order mark `TextDecoder` removes at the head of a stream
/// when `ignoreBOM` is false (its default, output-accumulator.ts:40).
const UTF8_BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];
```

#### 2. New stream-head filter state

Add above `OutputAccumulator`:

```rust
/// Stream-head BOM filter, mirroring `TextDecoder`'s default `ignoreBOM: false`
/// (output-accumulator.ts:40,70).
///
/// The BOM is removed **only** at the very start of the byte stream, so the state machine is
/// one-shot: it withholds a strict prefix of `EF BB BF` until the next byte decides, then latches
/// to [`BomFilter::Done`] and every subsequent byte passes through untouched (a second BOM, or a
/// BOM in the middle of the output, stays as a real `U+FEFF` — exactly like `TextDecoder`).
#[derive(Clone, Copy)]
enum BomFilter {
    /// The stream so far is exactly `UTF8_BOM[..n]` for `n < 3`; those `n` bytes are withheld from
    /// the decoded counters and from the preview tail. Since the withheld bytes are by definition
    /// a prefix of `UTF8_BOM`, `n` alone reconstructs them — nothing else needs storing.
    Matching(usize),
    /// The head has been decided (BOM consumed, or the first byte proved it was not a BOM).
    Done,
}
```

Add the field to the struct, next to the existing decoder carry
([output.rs:35-37](../../../crates/cyrup-tools/src/output.rs)):

```rust
    /// Streaming UTF-8 decoder carry: trailing bytes of an INCOMPLETE multibyte sequence held for
    /// the next chunk (mirrors `TextDecoder.decode(..., { stream: true })`).
    pending: Vec<u8>,
    /// Stream-head BOM removal state (mirrors `TextDecoder`'s default `ignoreBOM: false`). Applies
    /// to the DECODED path and the preview tail only — `total_raw_bytes` and the spill file keep
    /// the BOM, exactly like Pi (output-accumulator.ts:69,74-77).
    bom: BomFilter,
```

and to `OutputAccumulator::new` ([output.rs:45-62](../../../crates/cyrup-tools/src/output.rs)),
immediately after `pending: Vec::new(),`:

```rust
            pending: Vec::new(),
            bom: BomFilter::Matching(0),
```

#### 3. The filter itself

Add as a private method on `OutputAccumulator`, directly above `decode_into_counters`:

```rust
    /// Feed a raw chunk through the stream-head BOM filter and return the bytes the DECODED path
    /// and the preview tail should see (Pi: the output of `decoder.decode(chunk, {stream:true})`
    /// minus the leading BOM, output-accumulator.ts:40,70).
    ///
    /// Zero-copy in the only case that matters at runtime — once the head is decided the chunk is
    /// borrowed straight through. The single allocation happens at most once per accumulator, for
    /// the one chunk that ends a partially-matched BOM prefix with a non-BOM byte.
    fn filter_bom<'a>(&mut self, chunk: &'a [u8]) -> Cow<'a, [u8]> {
        let BomFilter::Matching(mut matched) = self.bom else {
            return Cow::Borrowed(chunk);
        };
        let mut rest = chunk;
        while matched < UTF8_BOM.len() {
            let Some((&b, tail)) = rest.split_first() else {
                // Chunk exhausted while the stream head is still a strict BOM prefix: keep
                // withholding, exactly like `TextDecoder` holding an undecided sequence.
                self.bom = BomFilter::Matching(matched);
                return Cow::Borrowed(&[]);
            };
            if UTF8_BOM.get(matched) != Some(&b) {
                self.bom = BomFilter::Done;
                if matched == 0 {
                    // Hot path: the stream simply does not start with a BOM — borrow, never copy.
                    return Cow::Borrowed(rest);
                }
                // A partial match that turned out not to be a BOM: release the withheld prefix
                // (which is, by construction, `UTF8_BOM[..matched]`) ahead of the rest.
                let mut out = Vec::with_capacity(matched + rest.len());
                out.extend_from_slice(UTF8_BOM.get(..matched).unwrap_or_default());
                out.extend_from_slice(rest);
                return Cow::Owned(out);
            }
            matched += 1;
            rest = tail;
        }
        // Full `EF BB BF` matched: drop it and forward the remainder of this chunk.
        self.bom = BomFilter::Done;
        Cow::Borrowed(rest)
    }
```

`UTF8_BOM.get(..)` / `split_first` are used deliberately: `indexing_slicing = "deny"` is on in
[Cargo.toml:101](../../../Cargo.toml), and `unwrap_or_default()` on `Option<&[u8]>` keeps
`unwrap_used = "deny"` satisfied.

#### 4. Route `append` through the filter

Current ([output.rs:155-181](../../../crates/cyrup-tools/src/output.rs)):

```rust
    /// Append a raw chunk (called from the `ProcOps::exec` data callback).
    pub fn append(&mut self, chunk: &[u8]) {
        if chunk.is_empty() {
            return;
        }
        self.total_raw_bytes += chunk.len();
        // Decode through the streaming UTF-8 decoder so totals/line-counts/last-line bytes reflect
        // the DECODED text (Pi parity, UM-8). For valid UTF-8 this equals the raw counts.
        self.decode_into_counters(chunk);

        // Rolling tail for the preview.
        self.buf.extend_from_slice(chunk);
        if self.buf.len() > self.cap {
            let start = self.buf.len() - self.cap;
            self.buf.drain(..start);
        }

        // Full output: buffer in memory until a limit is exceeded, then spill (and replay).
        if self.temp_file.is_some() || self.should_use_temp_file() {
            self.ensure_temp_replay();
            if let Some(file) = self.temp_file.as_mut() {
                let _ = file.write_all(chunk);
            }
        } else {
            self.raw_chunks.push(chunk.to_vec());
        }
    }
```

Replacement:

```rust
    /// Append a raw chunk (called from the `ProcOps::exec` data callback).
    ///
    /// Pi splits this chunk into a RAW path and a DECODED path (output-accumulator.ts:64-78) and a
    /// leading BOM survives on the raw side only: `totalRawBytes` counts it (:69) and the spill
    /// file/`rawChunks` keep it byte-for-byte (:74-77), while `TextDecoder`'s default
    /// `ignoreBOM: false` removes it before `appendDecodedText` ever runs (:40,70). Mirror that
    /// split exactly.
    pub fn append(&mut self, chunk: &[u8]) {
        if chunk.is_empty() {
            return;
        }
        // RAW path: the BOM counts here, and it still gates `should_use_temp_file` (Pi :69,205-208).
        self.total_raw_bytes += chunk.len();

        // DECODED path: everything the model can see goes through the stream-head BOM filter first.
        let visible = self.filter_bom(chunk);
        let visible = visible.as_ref();
        if !visible.is_empty() {
            // Decode through the streaming UTF-8 decoder so totals/line-counts/last-line bytes
            // reflect the DECODED text (Pi parity, UM-8). For valid UTF-8 this equals the raw
            // counts minus any stream-head BOM.
            self.decode_into_counters(visible);

            // Rolling tail for the preview (Pi `tailText`, built from decoded text, :155).
            self.buf.extend_from_slice(visible);
            if self.buf.len() > self.cap {
                let start = self.buf.len() - self.cap;
                self.buf.drain(..start);
            }
        }

        // Full output: buffer in memory until a limit is exceeded, then spill (and replay). The
        // ORIGINAL chunk, BOM included — Pi writes the raw `Buffer` (:74-77).
        if self.temp_file.is_some() || self.should_use_temp_file() {
            self.ensure_temp_replay();
            if let Some(file) = self.temp_file.as_mut() {
                let _ = file.write_all(chunk);
            }
        } else {
            self.raw_chunks.push(chunk.to_vec());
        }
    }
```

The `let visible = visible.as_ref();` reborrow matters: `filter_bom` returns a `Cow<'a, [u8]>` whose
lifetime is tied to `chunk`, **not** to `&mut self`, so the mutable borrow of `self` ends at the
call and the subsequent `self.decode_into_counters(..)` / `self.buf` uses compile cleanly.

#### 5. Release a never-completed BOM prefix in `finish`

A stream that ends mid-prefix was never a BOM. `TextDecoder`'s final `decode()`
([pi output-accumulator.ts:85](../../../tmp/pi/packages/coding-agent/src/core/tools/output-accumulator.ts))
turns `EF` or `EF BB` into one `U+FFFD` (3 decoded bytes), so the withheld bytes must be released
into the existing decoder carry **before** the carry is flushed.

Current ([output.rs:109-116](../../../crates/cyrup-tools/src/output.rs)):

```rust
    /// Flush any incomplete trailing sequence as a replacement char (Pi `decoder.decode()` with no
    /// `stream` flag, output-accumulator.ts:85). Idempotent. Call before reading final totals.
    pub fn finish(&mut self) {
        if !self.pending.is_empty() {
            self.pending.clear();
            self.append_decoded_text("\u{FFFD}");
        }
    }
```

Replacement:

```rust
    /// Flush any incomplete trailing sequence as a replacement char (Pi `decoder.decode()` with no
    /// `stream` flag, output-accumulator.ts:85). Idempotent. Call before reading final totals.
    pub fn finish(&mut self) {
        // A stream that ended while still inside a BOM prefix (`EF`, or `EF BB`, and nothing else)
        // never carried a BOM: release the withheld bytes into the decoder and the preview tail so
        // the final no-stream `decode()` renders them as one U+FFFD, exactly like Pi.
        if let BomFilter::Matching(matched) = self.bom {
            self.bom = BomFilter::Done;
            if matched > 0 {
                let held = UTF8_BOM.get(..matched).unwrap_or_default().to_vec();
                self.decode_into_counters(&held);
                self.buf.extend_from_slice(&held);
            }
        }
        if !self.pending.is_empty() {
            self.pending.clear();
            self.append_decoded_text("\u{FFFD}");
        }
    }
```

Latching `self.bom = BomFilter::Done` first preserves the documented idempotence, and it also means
`finalize` ([output.rs:233-252](../../../crates/cyrup-tools/src/output.rs)), which calls `finish()`
as its first statement, needs no change at all. No `cap` re-trim is needed after the release: at most
two bytes are appended, and `cap` is at least 8192
([output.rs:48](../../../crates/cyrup-tools/src/output.rs)).

---

## Concurrency notes

The accumulator is `!Sync`-by-construction single-owner state: it is created at
[bash.rs:262](../../../crates/cyrup-tools/src/tools/bash.rs) and moved into the `flush_fut` branch of
the `tokio::join!` at [bash.rs:387](../../../crates/cyrup-tools/src/tools/bash.rs), while
`exec_fut` only ever sends `Vec<u8>` over the `tokio::sync::mpsc::unbounded_channel`
([bash.rs:326-346](../../../crates/cyrup-tools/src/tools/bash.rs)). `filter_bom` therefore adds no
synchronisation requirement — it is `&mut self` state advanced from the single task that owns the
accumulator, in strict channel order. Nothing here may be reworked into `Arc<Mutex<..>>`: chunk
ordering is what makes "stream head" meaningful, and the existing single-consumer channel is what
guarantees it. Do not add locks, do not `spawn` the filter, and do not make `append` `async`.

The mid-stream reader `build_stream_update`
([bash.rs:568-592](../../../crates/cyrup-tools/src/tools/bash.rs)) runs on the same task between
`recv()` calls, so it always observes a consistent filter state; a preview taken while the head is
still undecided legitimately shows the withheld bytes as absent, which is exactly what `TextDecoder`
does with an undecided sequence.

---

## Out of scope (verified, do not change here)

[cyrup-session-svc/src/bash.rs:271-437](../../../crates/cyrup-session-svc/src/bash.rs) has its own
`BashOutputBuffer` for immediate-bash, whose `sanitize_binary_output` keeps `U+FEFF` (it only drops
C0 controls and `U+FFF9..=U+FFFB`), while its pi counterpart
[pi bash-executor.ts:76,82](../../../tmp/pi/packages/coding-agent/src/core/bash-executor.ts) also
uses a default `TextDecoder`. The same gap exists there. It is a **different** seam with a different
pi source file and is **not** part of this task; do not touch that file.

---

## Definition of done

Observable behaviour of the `bash` tool after the change:

1. A command whose output begins with `EF BB BF` returns content whose first character is the first
   real character of the output — no leading `U+FEFF` — in the returned result content, in the
   mid-stream `onUpdate` content, and in the final settle update.
2. `truncation.totalBytes` for such a command is 3 lower than the raw byte count of the process
   output; `truncation.totalLines` and the `[Showing last … of line N (line is …)]` partial-line
   footer are computed from the BOM-free text.
3. The spilled full-output temp file, when one is created, still begins with the raw `EF BB BF`
   bytes, and the decision to create it still accounts for those 3 raw bytes.
4. A BOM delivered split across chunk boundaries (`EF` then `BB BF`, or `EF BB` then `BF`, in any
   split including one byte per chunk) is removed just as completely as a BOM delivered whole.
5. A BOM anywhere other than the first three bytes of the combined stdout+stderr stream is preserved
   as a real `U+FEFF` — including a second BOM immediately following the stripped one.
6. Output that begins with a byte sequence that merely starts like a BOM (`EF BB 41`, or a lone
   `EF`) loses nothing: those bytes still reach the decoded text and are rendered as `U+FFFD`
   replacement characters with their usual 3-decoded-bytes-each weight.
7. Output that contains no BOM at all is byte-identical to today's behaviour, and the common path
   performs no extra copy of the chunk.
8. Nothing outside `crates/cyrup-tools/src/output.rs` changes.
