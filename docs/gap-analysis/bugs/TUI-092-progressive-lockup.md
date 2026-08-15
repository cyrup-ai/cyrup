# TUI-092 — The TUI degrades from smooth to a total lockup that cannot be exited

> **Task file for** the `TUI-092` row in [`../07-cyrup-tui.md`](../07-cyrup-tui.md) (filed 2026-08-15
> from live use, escalated to **critical** the same day). **No code has been changed; this is a
> triage spec, not a diagnosis.**
>
> **Status** — **NOT diagnosed.** No reproduction, no confirmed mechanism. Everything below marked
> *candidate* is a hypothesis with a stated test, not a finding. Do not implement from this file
> until §3 produces a measurement.
>
> **Kind** port-bug *(provisional — re-class once the mechanism is known; if it proves to be the
> same `live_floor` residue as `TUI-090`, it is `cyrup-original` and this file folds into that one)*
> · **Severity** **critical** · **Effort** **M** · **Confidence** **low — symptom only**
>
> **Cross-references** — [`TUI-090`](TUI-090-post-turn-whitespace.md) (**diagnosed, confirmed**;
> the leading candidate for *causing* this bug — see §2.1), `TUI-088` (Ctrl+C has no global
> binding — the reason this bug has no escape hatch), `TUI-091` (reasoning never renders; already
> believed a duplicate of `TUI-090`).

---

## 1. Symptom

Reported by the project owner from live use, in three increments:

1. *"the terminal is super fast and smooth but freezes up over time"*
2. *"keystrokes become unresponsive and rendering comes to a crawl"*
3. *"terminal gets totally locked up and can't even be killed with ctrl+d"*

**The progression matters and should not be collapsed.** It is not a single stall: the session
starts responsive and degrades monotonically with use, ending fully wedged.

### 1.1 The two facts that constrain the search

**(a) Keystrokes die *with* rendering, not after it.** If render cost alone were growing, input
would still be READ and the screen would simply lag behind it. Input going unresponsive means the
loop that reads keys is **starved**. So the target is render work executing *on the input path*, or
a lock the input path needs being held across a growing render — **not** an expensive draw.

**(b) `Ctrl+D` does not kill it either.** EOF on stdin failing to terminate the process means the
input path is not merely slow, it is **not being serviced at all** at that point. Combined with
`TUI-088` (no global `Ctrl+C` binding), the user has **no in-band way to exit** — the only recourse
is killing the process from another terminal. That elevates `TUI-088` beyond its own severity: it
is the missing escape hatch for this bug.

A hang that survives both EOF and the interrupt key is a **blocked event loop or a held lock**, not
slow drawing.

---

## 2. Candidates

Ordered by cost to test, cheapest first. **None is confirmed.**

### 2.1 Candidate A — `TUI-090` is the cause, not a sibling *(leading)*

`TUI-090` is **confirmed**: `live_floor` × `Terminal::insert_before` leaves a full screen of
whitespace after every turn, and committed content goes to native scrollback invisibly.

If the inline viewport's active region grows with each turn rather than collapsing back to content
size, then per-frame work grows **linearly with turn count** — which is exactly "fast at first,
crawling later." Whether that also starves input depends on whether the render runs on the same
task as the key reader; §3.2 settles that.

**This is the first thing to test, and if it holds, `TUI-092` should not be fixed independently.**

### 2.2 Candidate B — a per-turn leak

A subscription, task, channel or lock guard created per turn and never released. This codebase has
shipped that class **twice**: a `DrainLatch` that left the event bus permanently disabled after an
abort, and a dedup owner dropped without settling (fixed by making the owner hold the sole
`watch::Sender`).

Note `active_tools.iter` runs **five times per frame** (`transcript.rs`), so a single retained entry
costs every subsequent frame — a leak here degrades render *and* grows the window in which any lock
around it is held.

### 2.3 Candidate C — recomputation over history

Markdown wrapping or image rasterisation recomputed across accumulated history rather than cached.
`MAX_RASTER_PX` caps a single raster's size but not the number of them.

### 2.4 Already ruled out — do not re-check

- **`TranscriptView::pending`** is emptied by `drain_committed` via `std::mem::take`
  (`transcript.rs:553-556`).
- **`active_tools`** is drained at `transcript.rs:899` and `:935`.

Neither is an unbounded accumulator.

---

## 3. Reproduction and measurement — do this before anything else

**This bug has no reproduction yet. Producing one is the task**, and the file should be updated with
the result before any fix is designed.

### 3.1 Establish the curve

Drive a session to N turns and record **frame time** and **RSS** against turn count. If frame time
climbs roughly linearly with turns, Candidate A is live and `TUI-090`'s fix should be applied and
re-measured **before** investigating further. If RSS climbs without frame time, look at B.

### 3.2 Settle the starvation question

Determine whether the render and the key reader share a task. Instrument the input path with a
timestamp per key event and the render with per-frame duration, then compare: if key timestamps
gap exactly across long frames, render is blocking input directly. If keys stop while frames
continue, a lock is the culprit.

### 3.3 Capture the wedged state

When it locks, capture a backtrace of every thread (`sample <pid>` on macOS, or attach `lldb`). A
blocked event loop and a deadlock look identical from outside and completely different in a
backtrace. **This single capture is worth more than any amount of reasoning from the source.**

### 3.4 Rules

- **A real terminal is required.** `TestBackend` cannot express frame time, RSS, input latency, or
  "the viewport did not shrink". It is the reason `TUI-090` survived 96 rows of static analysis and
  a full assembled-render suite.
- **Instrument once and read**, per [`../handoff/03-verification.md`](../handoff/03-verification.md).
  Do **not** re-run repeatedly to characterise the hang — that method cost this project a day on a
  "deadlock" that turned out to be a network call.

---

## 4. Why the existing suite never caught it

Every TUI test drives a fixed-size `TestBackend` for a small number of turns. The suite therefore
cannot represent any of this bug's observables: elapsed frame time, memory growth, input
responsiveness, or a viewport that fails to shrink. It is not that the tests are wrong — the harness
has **no way to encode the question**, which is the same structural blindness `TUI-090`'s file
documents.

Any fix must ship with a check that would fail on a regression. Given the above, that check is
unlikely to be a `TestBackend` unit test; the headless VT-replay harness `TUI-090` used
(`src/tests/inline_stacking.rs`) is the closer precedent.

---

## 5. Acceptance criteria

1. A **measured curve** of frame time and RSS against turn count, recorded in this file.
2. A **thread backtrace of the wedged process**, recorded in this file.
3. The mechanism named, with the code path, and this file's *Kind* and *Confidence* updated.
4. If it proves to be `TUI-090`: this file is closed as a duplicate and the row re-pointed — **not**
   fixed separately.
5. If independent: a fix plus a regression check that fails without it, and a live-terminal session
   of ≥50 turns showing flat frame time and responsive input.
6. `TUI-088` closed alongside, or explicitly deferred with the owner's agreement — while this bug
   exists, the interrupt key is the only escape from it.

---

## 6. Evidence appendix

Owner reports, verbatim, 2026-08-15 live use:

```
the terminal is super fast and smooth but freezes up over time
keystrokes become unresponsive and rendering comes to a crawl
terminal gets totally locked up and can't even be killed with ctrl+d
which is currently the only way to exit the terminal since ctrl+c is borked up
```

Ruled-out accumulators, verified at HEAD:

```
transcript.rs:553-556   drain_committed → std::mem::take(&mut self.pending)
transcript.rs:899, :935 self.active_tools.drain(..)
```
