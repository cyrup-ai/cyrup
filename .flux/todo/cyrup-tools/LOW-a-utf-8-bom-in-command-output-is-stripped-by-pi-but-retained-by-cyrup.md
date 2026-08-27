---
title: A UTF-8 BOM in command output is stripped by pi but retained by cyrup
priority: LOW
tool: bash
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: qa
status: needs-rework
updated: 2026-08-27 14:06
---

# A UTF-8 BOM in command output is stripped by pi but retained by cyrup

## What is already DONE (production quality — do not touch)

The stream-head BOM filter is fully implemented and correct in
[`crates/cyrup-tools/src/output.rs`](../../../crates/cyrup-tools/src/output.rs):

- `UTF8_BOM` const (`:17`), `BomFilter` one-shot state enum (`:19-34`), `bom` field (`:60-63`),
  init to `BomFilter::Matching(0)` (`:84`).
- `filter_bom` (`:99-137`) — chunk-straddling, zero-copy on the no-BOM hot path, releases a
  partially-matched prefix ahead of the rest when the head turns out not to be a BOM.
- `append` (`:233-274`) — RAW path (`total_raw_bytes`, `raw_chunks`/temp-file spill) sees the
  untouched chunk; DECODED path (`decode_into_counters` + preview `buf`) sees `filter_bom` output.
- `finish` (`:176-194`) — releases a never-completed BOM prefix into the decoder carry before the
  final flush, latching `bom = Done` first so idempotence and `finalize` are unaffected.

Behaviour was verified by reading the code against every observable clause: leading BOM absent from
`tail_string`/`total_bytes`/`current_line_bytes` and therefore from both `bash.rs` consumers
(`bash.rs:511-515` final result, `bash.rs:691-695` `build_stream_update`); spill file and
`should_use_temp_file` still count the BOM; split BOMs handled; second/mid-stream BOM preserved;
`EF BB 41` and lone `EF` release correctly as `U+FFFD`. Nothing outside `output.rs` changed.

**Do not re-implement, refactor, or "improve" any of the above.**

---

## What REMAINS: regression tests

The change ships with **zero** tests. `grep -rn "bom\|Bom" crates/cyrup-tools/src/tests/
crates/cyrup-tools/src/output.rs` finds no test exercising `BomFilter` — the only BOM test in the
crate is `edit_unique_crlf_bom_diff` (`src/tests/tools.rs:317`), which covers the unrelated
`edit_diff::strip_bom` path.

This is out of step with the file's own convention: the previous parity fix at the same seam
(decoded-vs-raw byte counting) landed with `truncation_decision_uses_decoded_not_raw_bytes`
(`output.rs:396-422`), a test that names the pi lines it mirrors and asserts the exact byte diff.
Every clause below is a pure-unit assertion on `OutputAccumulator` — no process spawn, no async.

### Where

`mod tests` in [`crates/cyrup-tools/src/output.rs`](../../../crates/cyrup-tools/src/output.rs)
(`:348-442`). The module is already `#[allow(clippy::unwrap_used, clippy::indexing_slicing)]` and
`use super::*`, so `acc.buf`, `acc.temp_path`, `UTF8_BOM` and `BomFilter` are all in scope. Match the
existing style: one `#[test]` per behaviour, doc-comment/inline comment citing the pi line the
assertion mirrors.

### Required cases (one test each, or tightly grouped)

1. **Whole BOM at stream head is invisible to the decoded path.**
   `append(b"\xEF\xBB\xBFhi\n")`, `finish()` → `total_bytes() == 3` (`"hi\n"`), `total_lines() == 1`,
   `tail_string() == "hi\n"` (assert the string, not just the length — this is the model-visible
   clause), and `acc.total_raw_bytes == 6` (raw still counts the BOM, pi `:69`).

2. **Split delivery.** Parametrise over every split of `EF BB BF` plus a payload — at minimum
   `[EF][BB BF]`, `[EF BB][BF]`, and one byte per `append` call (`[EF][BB][BF]["hi"]`). Each must end
   with `tail_string() == "hi"` and `total_bytes() == 2`. A loop over the split points inside one
   `#[test]` is fine; assert with a message naming the split so a failure is diagnosable.

3. **Only the head is stripped.** `append("\u{feff}a\u{feff}b".as_bytes())` → `tail_string()` is
   `"a\u{feff}b"` and `total_bytes() == 5`; and a back-to-back double BOM
   (`b"\xEF\xBB\xBF\xEF\xBB\xBFx"`) keeps the second one: `tail_string() == "\u{feff}x"`.

4. **BOM-lookalike prefixes lose nothing.** `append(b"\xEF\xBB\x41")`, `finish()` → decoded text is
   `"\u{FFFD}A"`, `total_bytes() == 4`. And `append(b"\xEF")` with nothing else, `finish()` →
   `total_bytes() == 3` (exactly one `U+FFFD`, pi's final no-`stream` `decode()` at
   `output-accumulator.ts:85`) and `tail_string() == "\u{FFFD}"`. Also cover `EF BB` alone at
   end-of-stream → the same single `U+FFFD`, since it exercises the `matched == 2` release path in
   `finish`. Add the `EF BF BD` case (a literal `U+FFFD`, first byte matches `UTF8_BOM[0]`,
   second does not) → `tail_string() == "\u{FFFD}"`, `total_bytes() == 3`, proving the partial
   release in `filter_bom` reassembles the byte it withheld.

5. **Spill keeps the BOM verbatim, and the raw count still gates the spill.** Build an accumulator
   with a small `max_bytes` (mirror `spills_when_truncated_and_replays_buffered_chunks`,
   `output.rs:377-394`), feed a BOM-prefixed payload, `finalize(..)`, then read the returned temp
   file **as bytes** (`std::fs::read`, not `read_to_string`) and assert it starts with
   `[0xEF, 0xBB, 0xBF]` and equals the full raw input. Remove the temp file at the end like the
   neighbouring tests do. Include the boundary flavour: a payload whose raw length exceeds
   `max_bytes` *only because of* the 3 BOM bytes must still spill (pi `:69,205-208`).

6. **`finish()` stays idempotent.** Call `finish()` twice after a mid-prefix end (`append(b"\xEF")`)
   and assert `total_bytes()` is 3 both times — the `bom = Done` latch at `output.rs:183` is load
   bearing and nothing currently guards it against regression.

7. **No-BOM output is unchanged.** A short assertion that a plain `append(b"hello\n")` still yields
   `total_bytes() == 6`, `tail_string() == "hello\n"` — cheap insurance that the filter never eats a
   leading byte on the hot path.

### Constraints

- **Only** `crates/cyrup-tools/src/output.rs` may change, and only its `mod tests`. No production
  code edits — the implementation is correct as written.
- Do **not** touch `crates/cyrup-session-svc/src/bash.rs` (`BashOutputBuffer` is a different seam,
  explicitly out of scope) or `crates/cyrup-tools/src/tools/edit_diff.rs`.
- No `Arc<Mutex<..>>`, no `spawn`, no `async` in these tests — the accumulator is single-owner
  `&mut self` state and chunk ordering is the whole point.
- Run `cargo test -p cyrup-tools` and leave the suite green (292 tests passing at baseline, plus the
  new ones).
