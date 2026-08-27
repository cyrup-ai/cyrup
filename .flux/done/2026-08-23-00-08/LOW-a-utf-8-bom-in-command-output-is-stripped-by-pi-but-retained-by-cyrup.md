---
title: A UTF-8 BOM in command output is stripped by pi but retained by cyrup
priority: LOW
tool: bash
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: qa
status: completed
updated: 2026-08-27 20:07
---

# A UTF-8 BOM in command output is stripped by pi but retained by cyrup

## What is already DONE (production quality — DO NOT TOUCH)

The stream-head BOM filter is fully implemented and **QA-verified correct** (8/10, all eight
Definition-of-Done clauses PASS against real code) in
[`crates/cyrup-tools/src/output.rs`](../../../crates/cyrup-tools/src/output.rs):

| Symbol | Line (drifts — anchor by symbol) | Role |
| --- | --- | --- |
| `UTF8_BOM` | `:17` | `[0xEF, 0xBB, 0xBF]` |
| `enum BomFilter` | `:26-34` | one-shot `Matching(usize)` / `Done` |
| `OutputAccumulator.bom` field | `:60-63` | state, init `Matching(0)` at `:84` |
| `OutputAccumulator::filter_bom` | `:106-137` | chunk-straddling strip, zero-copy hot path, partial-prefix release |
| `OutputAccumulator::append` | `:240-274` | RAW path keeps BOM / DECODED path goes through `filter_bom` |
| `OutputAccumulator::finish` | `:178-194` | releases a never-completed prefix, latches `bom = Done` first |
| `OutputAccumulator::should_use_temp_file` | `:93-97` | still gated by `total_raw_bytes` (BOM included) |

Pi reference: [`output-accumulator.ts`](../../../tmp/pi/packages/coding-agent/src/core/tools/output-accumulator.ts)
— `new TextDecoder()` with default `ignoreBOM: false` (`:40`), `totalRawBytes += data.length` (`:69`),
`decoder.decode(data, {stream:true})` (`:70`), raw `Buffer` written to the spill / `rawChunks`
(`:74,76`), final no-`stream` `decoder.decode()` (`:85`), `totalBytes = totalDecodedBytes` (`:105`),
`shouldUseTempFile` OR-ing raw ‖ decoded ‖ lines (`:205-207`).

**Do not re-implement, refactor, or "improve" any of the above. No production code may change.**

---

## What REMAINS (the entire deliverable): regression tests

The change shipped with **zero** tests. `grep -rn "bom\|Bom\|BOM" crates/cyrup-tools/src/output.rs
crates/cyrup-tools/src/tests/` finds nothing exercising `BomFilter` — the only BOM tests in the crate
are `edit_unique_crlf_bom_diff` (`src/tests/tools.rs:317`) and `src/tests/edit_preview_diff.rs:61`,
both on the unrelated `edit_diff::strip_bom` path.

This is out of step with the file's own convention. The previous parity fix at *this exact seam*
(decoded-vs-raw byte counting) landed with `truncation_decision_uses_decoded_not_raw_bytes`
(`output.rs:396-422`): a doc-commented pure-unit test that names the pi lines it mirrors and asserts
the exact byte difference. **Every test below is that same shape** — construct, `append`, assert,
clean up. No process spawn, no async, no fixtures, no new dev-dependency.

### Where the tests go

Inside the existing `mod tests` in
[`crates/cyrup-tools/src/output.rs`](../../../crates/cyrup-tools/src/output.rs) (`:348-442`).
Append the new `#[test]` fns **after** `removes_file_when_not_truncated` (`:436-441`), i.e. before the
closing `}` of the module. Do not create a new file, a new module, or a `#[cfg(test)]` submodule.

Facts about that module you must rely on and must not change:

```rust
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]   // output.rs:348-349 — already present
mod tests {
    use super::*;                                          // output.rs:351
```

- `use super::*` puts `UTF8_BOM`, `BomFilter` and `OutputAccumulator` in scope.
- The tests are a child module, so **private fields are accessible**: `acc.buf`, `acc.temp_path`,
  `acc.cap`, `acc.total_raw_bytes`, `acc.pending`, `acc.bom`. Existing tests already read
  `acc.temp_path` (`:371,382`) and `acc.buf`/`acc.cap` (`:392`).
- The module allow-list covers `unwrap_used` and `indexing_slicing` **only**. Workspace lints
  (`Cargo.toml` root `[workspace.lints.clippy]`, `:97-101`) also `deny` `expect_used` and `panic`.
  Therefore: use `.unwrap()` (never `.expect(..)`), and use `assert!`/`assert_eq!` with a message
  (never a bare `panic!(..)`). Do **not** widen the `#[allow]` attribute.
- All five existing tests construct inline (`OutputAccumulator::new("cyrup-test", 2000, 1024)`) with
  no helper fn. **Match that — do not introduce a test helper or builder.** The `"cyrup-test"` prefix
  is the established temp-file prefix; reuse it.
- Every test that can create a temp file ends by removing it, as `spills_when_truncated_and_replays_buffered_chunks`
  does (`output.rs:393`). Follow that.

### The seven required tests — exact inputs, exact observables

Write these seven `#[test]` fns, in this order, with these names. Each carries a short doc-comment or
inline comment naming the pi line it mirrors, exactly like `:398-402`.

---

#### 1. `stream_head_bom_is_invisible_to_decoded_path`

Pins: BOM stripped from every model-visible counter; raw count keeps it (pi `:69` vs `:40,70`).

```rust
#[test]
fn stream_head_bom_is_invisible_to_decoded_path() {
    // Pi's `new TextDecoder()` defaults to `ignoreBOM: false` (output-accumulator.ts:40), so the
    // leading U+FEFF never reaches `appendDecodedText` (:70) — while `totalRawBytes` still counts
    // all 3 of its bytes (:69). 6 raw bytes in, 3 decoded bytes out.
    let mut acc = OutputAccumulator::new("cyrup-test", 2000, 1024);
    acc.append(b"\xEF\xBB\xBFhi\n");
    acc.finish();
    assert_eq!(acc.total_bytes(), 3, "decoded totals exclude the stream-head BOM");
    assert_eq!(acc.total_lines(), 1);
    assert_eq!(acc.last_line_bytes(), 0, "chunk ends on a newline");
    // The model-visible clause: assert the STRING, not just the length.
    assert_eq!(acc.tail_string(), "hi\n", "preview tail must not contain U+FEFF");
    assert_eq!(acc.total_raw_bytes, 6, "raw path keeps the BOM (pi :69)");
    assert!(acc.finalize(2000, 1024).is_none());

    // A stream that is nothing but a BOM decodes to the empty string.
    let mut only = OutputAccumulator::new("cyrup-test", 2000, 1024);
    only.append(&UTF8_BOM);
    only.finish();
    assert_eq!(only.total_bytes(), 0);
    assert_eq!(only.total_lines(), 0);
    assert_eq!(only.tail_string(), "");
    assert_eq!(only.total_raw_bytes, 3);
    assert!(only.finalize(2000, 1024).is_none());
}
```

#### 2. `bom_is_stripped_across_every_chunk_boundary`

Pins: the `Matching(n)` carry in `filter_bom` (`:111-133`) for every split of `EF BB BF` + payload.

Two loops in one `#[test]`. The first walks **every** split point of the 5-byte input
`EF BB BF 68 69`; the second delivers one byte per `append`. Every iteration must land on the same
observable, and the assert message must name the split so a failure is diagnosable.

```rust
#[test]
fn bom_is_stripped_across_every_chunk_boundary() {
    // `TextDecoder` with `stream: true` (output-accumulator.ts:70) holds an undecided head across
    // chunk boundaries; `BomFilter::Matching(n)` is that carry. Every split of `EF BB BF | "hi"`
    // must produce the identical decoded stream.
    const INPUT: &[u8] = b"\xEF\xBB\xBFhi";
    for split in 0..=INPUT.len() {
        let mut acc = OutputAccumulator::new("cyrup-test", 2000, 1024);
        acc.append(&INPUT[..split]);
        acc.append(&INPUT[split..]);
        acc.finish();
        assert_eq!(acc.tail_string(), "hi", "split at {split}");
        assert_eq!(acc.total_bytes(), 2, "split at {split}");
        assert_eq!(acc.total_raw_bytes, 5, "split at {split}: raw is split-invariant");
        assert!(acc.finalize(2000, 1024).is_none(), "split at {split}");
    }

    // Degenerate delivery: one byte per callback, the worst case for the carry.
    let mut acc = OutputAccumulator::new("cyrup-test", 2000, 1024);
    for b in INPUT {
        acc.append(&[*b]);
    }
    acc.finish();
    assert_eq!(acc.tail_string(), "hi", "byte-at-a-time delivery");
    assert_eq!(acc.total_bytes(), 2, "byte-at-a-time delivery");
    assert!(acc.finalize(2000, 1024).is_none());
}
```

#### 3. `only_the_stream_head_bom_is_stripped`

Pins: the one-shot `Done` latch (`:119,135`) — a mid-stream or second BOM stays a real `U+FEFF`.

```rust
#[test]
fn only_the_stream_head_bom_is_stripped() {
    // Pi strips at most one BOM, at offset 0 — every later U+FEFF is ordinary text
    // (output-accumulator.ts:40). `BomFilter::Done` is that one-shot latch.
    let mut mid = OutputAccumulator::new("cyrup-test", 2000, 1024);
    mid.append("\u{feff}a\u{feff}b".as_bytes()); // 8 raw bytes
    mid.finish();
    assert_eq!(mid.tail_string(), "a\u{feff}b");
    assert_eq!(mid.total_bytes(), 5, "3 stripped, the interior U+FEFF's 3 bytes kept");
    assert_eq!(mid.total_raw_bytes, 8);
    assert!(mid.finalize(2000, 1024).is_none());

    // Back-to-back BOMs: only the first goes.
    let mut double = OutputAccumulator::new("cyrup-test", 2000, 1024);
    double.append(b"\xEF\xBB\xBF\xEF\xBB\xBFx");
    double.finish();
    assert_eq!(double.tail_string(), "\u{feff}x", "the second BOM is real text");
    assert_eq!(double.total_bytes(), 4);
    assert!(double.finalize(2000, 1024).is_none());
}
```

#### 4. `bom_lookalike_prefixes_lose_no_bytes`

Pins: the partial-release branch of `filter_bom` (`:124-129`) and the `finish` release (`:182-189`).
Four sub-cases, each a distinct code path. **Do not merge them into a loop** — the expected outputs
differ in kind, and the `EF BF BD` case asserts raw bytes rather than a lossy string.

```rust
#[test]
fn bom_lookalike_prefixes_lose_no_bytes() {
    // `EF BB 41`: two BOM bytes withheld, then the third byte disproves the BOM. `filter_bom`
    // releases `UTF8_BOM[..2]` ahead of the rest, so the decoder sees `EF BB 41` and produces
    // U+FFFD (3 bytes, for the invalid `EF BB`) + "A" — exactly what pi's TextDecoder yields.
    let mut a = OutputAccumulator::new("cyrup-test", 2000, 1024);
    a.append(b"\xEF\xBB\x41");
    a.finish();
    assert_eq!(a.tail_string(), "\u{FFFD}A");
    assert_eq!(a.total_bytes(), 4, "U+FFFD(3) + 'A'(1)");
    assert_eq!(a.total_raw_bytes, 3);
    assert!(a.finalize(2000, 1024).is_none());

    // Lone `EF` at end of stream: never a BOM, never completed. `finish` releases it into the
    // decoder, whose final no-`stream` `decode()` (output-accumulator.ts:85) emits ONE U+FFFD.
    let mut lone = OutputAccumulator::new("cyrup-test", 2000, 1024);
    lone.append(b"\xEF");
    assert_eq!(lone.total_bytes(), 0, "withheld while still undecided");
    lone.finish();
    assert_eq!(lone.tail_string(), "\u{FFFD}");
    assert_eq!(lone.total_bytes(), 3, "exactly one U+FFFD, not two");
    assert!(lone.finalize(2000, 1024).is_none());

    // `EF BB` at end of stream: the `matched == 2` release path in `finish`.
    let mut two = OutputAccumulator::new("cyrup-test", 2000, 1024);
    two.append(b"\xEF\xBB");
    two.finish();
    assert_eq!(two.tail_string(), "\u{FFFD}");
    assert_eq!(two.total_bytes(), 3, "one U+FFFD for the incomplete sequence");
    assert_eq!(two.total_raw_bytes, 2);
    assert!(two.finalize(2000, 1024).is_none());

    // `EF BF BD` is a literal U+FFFD whose FIRST byte matches UTF8_BOM[0] and whose second does
    // not. Proves `filter_bom` reassembles the byte it withheld: if the withheld `EF` were
    // dropped, `BF BD` would decode to garbage instead of one clean character.
    let mut fffd = OutputAccumulator::new("cyrup-test", 2000, 1024);
    fffd.append(b"\xEF\xBF\xBD");
    assert!(fffd.pending.is_empty(), "decoded as one complete char, nothing carried");
    assert_eq!(fffd.buf, vec![0xEF, 0xBF, 0xBD], "withheld byte re-emitted verbatim");
    fffd.finish();
    assert_eq!(fffd.tail_string(), "\u{FFFD}");
    assert_eq!(fffd.total_bytes(), 3);
    assert!(fffd.finalize(2000, 1024).is_none());
}
```

#### 5. `spill_file_keeps_the_bom_and_raw_count_still_gates_the_spill`

Pins: pi `:74,76` (raw `Buffer` on the spill path) and `:205-207` (raw ‖ decoded ‖ lines).
Read the file **as bytes** (`std::fs::read`, never `read_to_string`) — a `String` compare cannot tell
a preserved BOM from a stripped one at a glance. Mirror the shape of
`spills_when_truncated_and_replays_buffered_chunks` (`output.rs:377-394`), including the
`std::fs::remove_file` at the end.

```rust
#[test]
fn spill_file_keeps_the_bom_and_raw_count_still_gates_the_spill() {
    // Pi writes the untouched `Buffer` to the spill (output-accumulator.ts:74,76): the full-output
    // file is a byte-exact copy of the process's stdout, BOM included.
    let mut acc = OutputAccumulator::new("cyrup-test", 2000, 16);
    acc.append(b"\xEF\xBB\xBF0123456789"); // 13 raw, under the 16-byte limit ⇒ buffered
    assert!(acc.temp_path.is_none());
    acc.append(b"abcdefghij"); // 23 raw ⇒ spill opens and replays chunk 1 WITH the BOM
    assert!(acc.temp_path.is_some());
    assert_eq!(acc.total_bytes(), 20, "decoded side still excludes the BOM");
    let p = acc.finalize(2000, 16).unwrap();
    let bytes = std::fs::read(&p).unwrap();
    assert_eq!(&bytes[..3], &UTF8_BOM[..], "spill file must start with EF BB BF");
    assert_eq!(bytes, b"\xEF\xBB\xBF0123456789abcdefghij".to_vec());
    let _ = std::fs::remove_file(&p);

    // Boundary flavour: 14 payload bytes + 3 BOM bytes = 17 raw > 16 = max, while decoded 14 <= 16.
    // The spill is triggered by `totalRawBytes` ALONE (pi :69,205-207) — the BOM must still count.
    let mut edge = OutputAccumulator::new("cyrup-test", 2000, 16);
    edge.append(b"\xEF\xBB\xBF0123456789abcd");
    assert_eq!(edge.total_bytes(), 14, "decoded is under the limit");
    assert_eq!(edge.total_raw_bytes, 17, "raw is over it, only because of the BOM");
    assert!(edge.is_truncated(), "raw count alone must trip the spill");
    let p = edge.finalize(2000, 16).unwrap();
    let bytes = std::fs::read(&p).unwrap();
    assert_eq!(bytes, b"\xEF\xBB\xBF0123456789abcd".to_vec());
    let _ = std::fs::remove_file(&p);
}
```

#### 6. `finish_is_idempotent_after_a_partial_bom`

Pins: the `bom = BomFilter::Done` latch at `output.rs:183` — set *before* the release, so a second
`finish()` (and the `finish()` that `finalize` performs at `:327`) cannot double-emit. Nothing
currently guards it.

```rust
#[test]
fn finish_is_idempotent_after_a_partial_bom() {
    // `finish` latches `bom = Done` BEFORE releasing the withheld prefix (output.rs:183). Without
    // that latch the second call would re-release `EF` and emit a second U+FFFD. `finalize` calls
    // `finish` again internally (:327), so this is a real path, not a hypothetical.
    let mut acc = OutputAccumulator::new("cyrup-test", 2000, 1024);
    acc.append(b"\xEF");
    acc.finish();
    assert_eq!(acc.total_bytes(), 3);
    assert_eq!(acc.buf.len(), 1);
    acc.finish();
    assert_eq!(acc.total_bytes(), 3, "second finish must not emit another U+FFFD");
    assert_eq!(acc.buf.len(), 1, "second finish must not re-release the prefix");
    assert_eq!(acc.tail_string(), "\u{FFFD}");
    assert!(acc.finalize(2000, 1024).is_none(), "finalize's internal finish is also a no-op");
    assert_eq!(acc.total_bytes(), 3);
}
```

#### 7. `no_bom_output_is_untouched`

Pins: the `matched == 0` zero-copy hot path (`:120-123`) — the overwhelmingly common case, where a
filter bug would silently eat the first byte of every command's output.

```rust
#[test]
fn no_bom_output_is_untouched() {
    // Hot path: the first byte (0x68) is not UTF8_BOM[0], so `filter_bom` borrows the whole chunk
    // straight through. Cheap insurance that the filter never eats a leading byte.
    let mut acc = OutputAccumulator::new("cyrup-test", 2000, 1024);
    acc.append(b"hello\n");
    acc.finish();
    assert_eq!(acc.tail_string(), "hello\n");
    assert_eq!(acc.total_bytes(), 6);
    assert_eq!(acc.total_raw_bytes, 6, "raw and decoded agree when there is no BOM");
    assert_eq!(acc.total_lines(), 1);
    assert!(acc.finalize(2000, 1024).is_none());
}
```

---

## Constraints

- **Only** `crates/cyrup-tools/src/output.rs` may change, and within it **only** `mod tests`
  (`:348-442`). Zero production-code edits — the implementation is QA-verified correct.
- Do **not** widen the module's `#[allow(clippy::unwrap_used, clippy::indexing_slicing)]`.
  `expect_used` and `panic` are workspace-denied; `.unwrap()` + `assert!` with a message cover
  everything above.
- Do **not** touch `crates/cyrup-session-svc/src/bash.rs` (`BashOutputBuffer` is a different seam,
  explicitly out of scope) or `crates/cyrup-tools/src/tools/edit_diff.rs`.
- No `Arc<Mutex<..>>`, no `spawn`, no `async`, no `tokio::test`. `OutputAccumulator` is single-owner
  `&mut self` state and chunk ordering is the entire point.
- No new dev-dependency, no fixture file, no test helper fn — construct inline like the five existing
  tests.
- Every test that can produce a temp file removes it (`std::fs::remove_file`), as
  `output.rs:393` and `:419-421` do.

## Definition of Done

1. Seven new `#[test]` fns exist in `mod tests` of `crates/cyrup-tools/src/output.rs`, named exactly:
   `stream_head_bom_is_invisible_to_decoded_path`, `bom_is_stripped_across_every_chunk_boundary`,
   `only_the_stream_head_bom_is_stripped`, `bom_lookalike_prefixes_lose_no_bytes`,
   `spill_file_keeps_the_bom_and_raw_count_still_gates_the_spill`,
   `finish_is_idempotent_after_a_partial_bom`, `no_bom_output_is_untouched`.
2. Each asserts the exact observables listed above — in particular `tail_string()` compared as a
   **string** (not a length) in cases 1-4 and 7, and the spill file compared as **bytes** in case 5.
3. Each carries a comment citing the pi line(s) it mirrors, matching the style of
   `truncation_decision_uses_decoded_not_raw_bytes` (`output.rs:396-422`).
4. `git diff --stat` touches exactly one file, `crates/cyrup-tools/src/output.rs`, and the diff
   contains no change above line 348 (the `#[cfg(test)]` marker).
5. `cargo test -p cyrup-tools` is green: the pre-existing tests still pass, plus the seven new ones.
6. `cargo clippy -p cyrup-tools --all-targets` is clean — no new warnings, and the module's
   `#[allow]` attribute is unchanged.
