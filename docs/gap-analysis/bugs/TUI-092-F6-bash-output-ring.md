# TUI-092-F6 — Bound `BashExecution::output_lines` to a rolling window with an omission counter

> **Part of** [`TUI-092-progressive-lockup.md`](TUI-092-progressive-lockup.md) (the umbrella audit).
> Self-contained change in `crates/cyrup-tui/src/bash.rs` plus one render row.
>
> **Kind** `cyrup-original` · **Severity** high · **Effort** S · **Phase driven** 2→3 during chatty
> runs (`! npm install` with 200k lines)

## Coordinates with

Nothing. Independent of F1–F5, F7, F8. Matches the executor's own truncation ethos
([`cyrup_tools::truncate::DEFAULT_MAX_LINES`](../../../crates/cyrup-tools/src/truncate.rs#L11) =
2000) — the live block makes the same trade the recorded result already makes.

---

## Evidence

[`bash.rs:45`](../../../crates/cyrup-tui/src/bash.rs#L45) +
[`append_output`](../../../crates/cyrup-tui/src/bash.rs#L121): every chunk the executor streams is
split and pushed, uncapped, for the block's whole lifetime. The session-side sink forwards **every**
sanitised chunk ([`cyrup-session-svc/src/bash.rs:179-184`](../../../crates/cyrup-session-svc/src/bash.rs#L179));
the rolling 100 KB cap (`ROLLING_MAX_BYTES`) applies to the *result preview*, not the live stream.
A `! npm install` with 200k lines accumulates 200k `String`s; the collapsed render shows the last
20 of them. On commit the whole vec renders into scrollback (and, pre-F1, into the test
accumulator).

**Cost shape.** memory ∝ run output, for the whole block lifetime.

---

## FIX — a bounded ring with an omission counter

```rust
// bash.rs
/// The live block retains at most the LAST `MAX_OUTPUT_LINES` output lines — the same bound Pi's
/// executor applies to the recorded result (`truncate.ts`); earlier lines are counted, not kept
/// (TUI-092 F6).
const MAX_OUTPUT_LINES: usize = 2000;

pub struct BashExecution {
    // …
    output_lines: std::collections::VecDeque<String>,
    /// Lines evicted from the front of `output_lines`, rendered as an omission notice.
    omitted_lines: usize,
    // …
}

pub fn append_output(&mut self, chunk: &str) {
    // … existing split/merge logic, pushing onto the VecDeque …
    while self.output_lines.len() > MAX_OUTPUT_LINES {
        self.output_lines.pop_front();
        self.omitted_lines += 1;
    }
}
```

`render_lines` (and the committed `Entry::Bash` arm in
[`entry_lines`](../../../crates/cyrup-tui/src/transcript.rs#L2750)) prepend one dim row when the
counter is non-zero: `… ({omitted_lines} earlier lines omitted) …`. The `Ctrl+O` expanded view
shows the retained window — the same trade the executor already makes for the recorded result.

---

## Definition of done

* **Every live collection has a stated bound.** `BashExecution::output_lines` ≤ 2000 lines + an
  omission counter; the eviction happens in `append_output` on every chunk.
* The omission row renders exactly once (when `omitted_lines > 0`), dim-styled, ahead of the retained
  window in both the collapsed and expanded views.

## Do not touch

The session-side sink's per-chunk forwarding (that is the live stream the TUI consumes; bounding
the *consumer* here is the fix, not the producer), and the rolling 100 KB cap on the *result
preview* in `cyrup-session-svc/src/bash.rs` (a separate, already-correct bound).