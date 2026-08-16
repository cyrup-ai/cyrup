# TUI-090 — A full screen of whitespace after every agent turn; the response is only in native scrollback

> **Task file for** the `TUI-090` row in [`../07-cyrup-tui.md`](../07-cyrup-tui.md) (filed 2026-08-15
> from live use, "architectural lead, to be confirmed not assumed"). This file is the confirmation
> and the fix specification. **No code has been changed; this is diagnosis + task spec only.**
>
> **Status** diagnosed — root cause **confirmed by headless reproduction** at cyrup HEAD `7e2e60c`;
> fix specified, not implemented.
>
> **Kind** cyrup-original *(re-class from `port-bug` at reconciliation: the mechanism that fails —
> `live_floor` × `Terminal::insert_before` — has no upstream counterpart; it is behavioural residue
> of an ADR-0001 substrate difference, ADR-0001 rule 4)* · **Severity** **high** · **Effort** **M**
> (the code change is S; the pty verification the last fix in this area already demanded and never
> got is the M) · **Confidence** **confirmed — reproduced headlessly with a VT-replay of the real
> backend byte stream** (the harness built for the stacking bug, `src/tests/inline_stacking.rs`).
>
> **Cross-references** — [`07-cyrup-tui.md`](../07-cyrup-tui.md) rows **TUI-090** (this bug) and
> **TUI-091** (reasoning blocks never render — **likely a duplicate symptom**: any committed block,
> reasoning included, goes to scrollback invisibly under this bug; re-check TUI-091 in a real
> terminal *after* this fix lands, before touching it). Commit `5900984`
> (`fix(tui): progressively commit finished tool calls + bound the live region`) introduced **both**
> interacting halves. Commit `72dc5de` (erase-before-reconstruct, the stacking fix) is a
> precondition the fix must keep. ADR-0001 commitment 5(a) governs the substrate.

---

## 1. Symptom

Owner report (live use, 2026-08-15): *after each agent turn a full screen of whitespace appears;
the user has to scroll up — into the scrollback above the viewport — to view the last agent
response.* Every substantive turn ends with the terminal showing blank rows plus the idle
editor/footer at the bottom; the reply to the message just sent is above the fold.

Measured headlessly (§3): after a 30-line turn on an 80×24 terminal, **0 of 30 response lines are
on the visible grid, all 30 are in scrollback, and 21 of 24 visible rows are blank.** The user must
scroll up more than a full screen — past ~31 blank scrollback rows the mechanism also emits — to
find the first line of the response.

What it is **not**: the viewport does *not* fail to collapse at turn end (TUI-090's hypothesised
shape). Measured: `viewport_height` goes 24 → 5 on the first idle draw. The collapse works; the
problem is that by the time it fires, **everything has already been flushed invisibly**, and the
collapse erases the whole screen with nothing left to put back above the idle viewport.

## 2. Root cause — confirmed mechanism

Five interacting pieces, each correct in isolation, composing into the bug. The load-bearing
conflict: **the grow-only `live_floor` keeps the inline viewport pinned at full-terminal height at
exactly the moments mid-turn commits flush, and ratatui's `insert_before` sends inserted lines
straight to native scrollback when the viewport fills the screen.**

1. **The viewport pins at a grow-only high-water mark for the whole turn** (the FLICKER fix,
   `5900984` PART 2). `App::draw` (`crates/cyrup-tui/src/app.rs:1893-1900`):
   `turn_active = status.streaming || transcript.has_bash()`; while active,
   `live_floor = live_floor.max(raw).min(term_h)` and `desired = live_floor`. The floor never
   decreases mid-turn, capped only at the terminal height. The run loop draws after **every**
   session event (`app.rs:8449-8475`, `maybe_ev = events.next()` → `draw_synchronized()`), so this
   pins continuously, not just at tool boundaries.

2. **Any turn with ≳ `term_h − chrome` rows of content drives the floor to full screen.**
   `region_constraints` (`app.rs:7167`) caps the message region at the rows remaining after
   editor/footer/band; `live_region_height` (`app.rs:7304`) sums them, so `raw` saturates at
   `term_h` and the floor follows. On a 24-row terminal that threshold is ~18 rows of streamed
   text/tool output — i.e. most real turns. Measured in the repro: viewport grows 7 → 24 by the
   20th streamed line and stays at 24.

3. **Mid-turn commits move content out of the live region while the floor is still pinned** (the
   SCREEN-FILL fix, `5900984` PART 1, plus message finalization):
   - `MessageEnd` → `finalize_assistant_message` (`app.rs:6035`) → `commit_thinking` +
     `commit_assistant` (`transcript.rs:636`) — the assistant text leaves the streaming buffer.
   - `ToolExecutionEnd` → `commit_finished_leading_tools` (`app.rs:5719`, `transcript.rs:926`) —
     finished tools leave `active_tools`.
   - `AgentEnd` → `commit_assistant(None)` + `commit_tools` (`app.rs:5559-5575`).
   Committed entries queue in `transcript.pending()` (`transcript.rs:409`) and the very next draw
   flushes them: `flush_committed` (`app.rs:1944`) → `Terminal::insert_before`.

4. **`insert_before` with a full-screen viewport inserts directly into scrollback — invisibly.**
   cyrup enables only ratatui's `unstable-rendered-line-info` feature
   (`crates/cyrup-tui/Cargo.toml:50`), so the active implementation is
   `insert_before_no_scrolling_regions` (`ratatui-core-0.1.2/src/terminal/inline.rs:109`). Its own
   doc contract: *"At the limit, if the viewport takes up the whole screen, all lines will be
   inserted directly into the scrollback buffer."* Mechanically it draws the lines at the top of
   the screen and then `scroll_up(drawn_height)` scrolls them off, all inside one synchronized
   update — the user never sees them. (The `scrolling-regions` feature's full-screen path has the
   same outcome; enabling it is **not** a fix.) So the response text committed at `MessageEnd`, and
   every tool block committed at `ToolExecutionEnd`, leaves the screen the moment it commits. The
   design intent recorded in the `ToolExecutionEnd` arm (`app.rs:5712-5718`) — *"lands it above the
   viewport on the very next frame … an atomic handoff, no duplicate/flash"* — is defeated whenever
   the floor is at `term_h`.

5. **The turn-end shrink erases the whole screen with nothing left to flush.** At `AgentEnd`,
   `turn_active` goes false → `live_floor = 0` → `desired = raw` (idle, ~5 rows). `resize_viewport`
   (`app.rs:1918`) calls `reanchor_inline(term_h, old_h = 24, new = 5)`;
   `reanchor_inline_region` (`app.rs:869-891`) computes `erase_top = term_h − old_h = 0` and emits
   `MoveTo(0,0)` + `Clear(FromCursorDown)` — **the entire screen is erased** — then re-anchors at
   row 19. `flush_committed` then has nothing (or only the last tool block) left to insert,
   because step 4 already flushed everything mid-turn. Final frame: ~19 blank rows + the 5-row
   idle viewport. The erase/scroll sequences also push blank rows into native scrollback (~31 in
   the repro), which is why the user scrolls up through even *more* whitespace before reaching the
   response.

**The trigger, precisely:** a mid-turn flush that happens while `live_floor == term_h` **and**
`raw < term_h` (the live tail no longer fills the screen once the committed content leaves). When
`raw == term_h` even after the commit (a huge still-running tool tail), an invisible flush is
seamless — the screen stays full of live tail. The catastrophic case is the *stale* floor: the
content shrank but the viewport didn't.

**Secondary defect, same root (medium turns that never reach full screen):** the mid-turn flush
lands directly above the *pinned* viewport (correct at that instant), but the `AgentEnd` shrink
then erases the rows the viewport vacates and nothing repaints them — a blank band opens between
the response tail and the editor (measured 11 rows on a 10-line turn). Additionally ratatui's
no-scrolling-regions insert top-anchor-packs: it scrolls `drawn + B + vh − S` rows even when the
insert fits without scrolling, pushing real content into scrollback earlier than necessary.

## 3. Reproduction (headless, deterministic)

`TestBackend` cannot express this bug (its grid never scrolls — the same blind spot named in the
TUI-090/TUI-091 rows and in the `inline_stacking.rs` module header). The repro therefore reuses
that file's harness: a real `CrosstermBackend` over a shared capture buffer whose `rebuild()`
re-wraps the **same** buffer (bytes accumulate like a physical terminal), replayed through a tiny
VT screen model (visible grid + scrollback).

Scenario driven (the exact calls the production event arms make, at the production cadence of one
draw per event): seed model → draw → submit "fix the bug" → `set_streaming(true)` (AgentStart) →
stream 30 lines with a draw each → `commit_assistant(None)` (MessageEnd) + draw → 3×
(`push_tool_start` / draw / `push_tool_end` / `commit_finished_leading_tools` / draw) →
`set_streaming(false)` + `commit_tools()` (AgentEnd) + draw ×2 → replay the captured bytes.

Measured output at HEAD `7e2e60c` (80×24):

```
viewport_height: 7 → 11 → 16 → 21 → 24 (by streamed line 20) → 24 (mid-turn commit) → 24 → 5 (AgentEnd)
VISIBLE GRID after the turn: rows 0-18 blank, row 19/21 editor rules, row 20 editor, row 23 footer
  → 21/24 visible rows blank
response lines on grid: 0/30 · response lines in scrollback: 30/30 (rows sb32-sb61)
scrollback rows sb0-sb31: blank (emitted by the growth/erase/flush scroll sequences)
```

Short-turn control (10 streamed lines, floor peaks at 16 < 24): 8/10 response lines visible — but
an **11-row blank band** sits between the response tail (row 7) and the editor (row 19) after
`AgentEnd` (the secondary defect, §2). The bug therefore has a continuous spectrum; the owner hit
the full-screen extreme because real agent turns (text + tools) almost always exceed
`term_h − chrome` rows.

The temporary test used for the measurement was reverted after the run; §6 specifies the permanent
regression tests to add in the same harness.

## 4. Why the existing suite never caught it

- `TestBackend` starts each `rebuild` from a blank grid and never scrolls; "flushed to scrollback"
  and "erased" are indistinguishable on it (stated in `inline_stacking.rs:1-20` and in both area
  rows). Every assembled-render test (`tests/render.rs`, `tests/assembled_render.rs`) runs on it.
- The unit guard for the floor, `live_floor_grows_then_holds_during_a_turn_and_resets_when_idle`
  (`app.rs:9221`), asserts the floor **holds across a mid-turn commit** — it enshrines the bug's
  precondition as expected behaviour. It must be re-specified, not just kept green.
- `5900984`'s own commit message ends: *"Definitive verification is a real pty agentic-turn
  drive."* That drive never happened; both prior user disasters in this area (SCREEN-FILL,
  STACKING) were likewise only caught live.

## 5. Constraints the fix must not regress

1. **ADR-0001 commitment 5(a)** — content-sized inline viewport; committed history reaches native
   scrollback via `Terminal::insert_before` **exactly once** (R-ARCH-TUI-003); no alternate screen
   on the default path. Committed entries are never re-rendered inside the viewport.
2. **FLICKER fix** (`5900984` PART 2) — no `Terminal` reconstruction per tool *event*; ratatui
   cell-diffs message churn inside a stable viewport.
3. **SCREEN-FILL fix** (`5900984` PART 1) — finished tools commit progressively; `content_height`
   stays bounded to the running tail.
4. **void-fix** — the idle viewport collapses to the compact editor/footer (`app.rs:1892`).
5. **Stacking fix** (`72dc5de`) — erase-before-reconstruct (`reanchor_inline_region`) must keep
   running before every rebuild; reconstructions must stay visually atomic (synchronized update).
6. **pi parity of intent** — pi's finished `ToolExecutionComponent`s scroll into native history
   *visibly* as the turn proceeds (`tool-execution.ts:13`; the `firstChanged < prevViewportTop`
   full-redraw rule, `tui.ts:1455` @v0.83.0). pi's invariant: the newest content is always
   bottom-anchored directly above the dock; scrolling is continuous and visible. That — not any
   ratatui mechanism — is the behavioural target.

## 6. Resolution path

### Recommended — flush-synchronized floor release (Path 1)

**Invariant restored:** *at every `insert_before`, the viewport is sized to the live content that
remains after the commit* — so the flushed lines land directly above the viewport and stay on
screen, and the viewport is never stale-full while the screen is not.

Change site: `App::draw` (`app.rs:1884-1904`). Today:

```rust
let turn_active = self.state.status.streaming || self.state.transcript.has_bash();
let desired = if turn_active {
    self.live_floor = self.live_floor.max(raw).min(term_h);
    self.live_floor
} else { self.live_floor = 0; raw };
```

Release the floor **only on frames that will flush**, and only down to `raw`:

```rust
let turn_active = self.state.status.streaming || self.state.transcript.has_bash();
let flush_pending = !self.state.transcript.pending().is_empty();   // accessor already exists
let desired = if turn_active {
    if flush_pending && raw < self.live_floor {
        self.live_floor = raw;            // content left the region: shrink BEFORE the flush
    }
    self.live_floor = self.live_floor.max(raw).min(term_h);
    self.live_floor
} else { self.live_floor = 0; raw };
```

`draw` already orders `resize_viewport` **before** `flush_committed` (`app.rs:1901-1905`, with the
comment that says exactly why), so a released floor produces shrink → flush in one frame.

Why this is right, case by case:

- **Full-screen turn (the reported bug):** at `MessageEnd`, `raw` drops 24 → ~7 (text left the
  region), the floor releases to 7, the viewport shrinks, and the 30 committed lines insert above
  a 7-row viewport: the top 13 scroll off, **lines 14-30 stay on screen** directly above the live
  region. Tool commits behave the same. At `AgentEnd` the final tail flushes above the idle
  viewport. No blank screen at any point; the response tail is visible when the turn settles.
- **Sub-full-screen turn:** at each commit the viewport shrinks to `raw` and the flush lands
  directly above it; the subsequent grow-only re-pin keeps the flushed lines glued to the viewport
  top (the growth scroll shifts both together — verified against `reanchor_inline_region`'s
  geometry). The 11-row band of the secondary defect collapses to at most the indicator-band rows
  that legitimately vanish at idle.
- **`raw == term_h` after a commit** (a huge still-running tail): the release does not fire
  (`raw < live_floor` is false), the flush goes to scrollback invisibly — and that is *seamless*,
  because the screen remains full of live tail; the committed rows were already above the
  auto-scrolled window (`transcript.rs:3248-3267`). This is pi's continuous-scroll behaviour.
- **Cost:** one shrink reconstruction per *commit* (tool end / message end / bash done) — not per
  event. Growth between commits reconstructs exactly as it does today (grow-only tracking already
  reconstructs per completed wrapped line while streaming). The erase-before-reconstruct +
  synchronized-update pair keeps each reconstruction atomic. This is a deliberately bounded,
  partial re-tightening of the FLICKER fix: shrinking is restored **only** at the frames where
  content leaves the region, which are precisely the frames where a shrink is visually required.
- **Edge cases to keep:** the `has_bash()` arm (a `!` block commits via `commit_bash`, same
  release path); the session-replay commit at `app.rs:1753` (replay runs idle, floor already 0 —
  unaffected); `/new` reset (`app.rs:1541` already zeroes the floor); aborted turns (the `AgentEnd`
  arm handles them, `app.rs:5567-5572`).

### Alternative — full-screen flush deferral (Path 2, fallback)

Gate the mid-turn flush (not the commit) on `viewport_height < term_h`: while full-screen, leave
entries in `pending` **and keep rendering them in the live region** (decouple "committed" from
"removed from the live render"; `content_height` includes them; auto-scroll shows the tail).
Everything flushes at the first sub-full-screen frame — at latest the `AgentEnd` shrink — visibly.
Closest to pi's model (pi never removes components from the render tree), and adds zero
reconstructions. Costs: a real `TranscriptView` change (`lines()`/`content_height`/drain
semantics), and the exactly-once invariant must be re-proven across session-replace and abort
paths. Choose this only if the pty drive shows Path 1's per-commit shrink is visually
unacceptable.

### Rejected

- **Path 3 — reprint the turn tail at `AgentEnd`.** The content is already in native scrollback;
  re-inserting duplicates it for anyone who scrolls up. Violates exactly-once visibly.
- **Path 4 — cap the floor at `term_h − K`.** With viewport `S − K`, ratatui's insert leaves only
  the last **K** lines of each flush on screen; no fixed K guarantees the tail, and the turn-end
  vacated-rows band is untouched. Insufficient on both defects.
- **Path 5 — revert progressive commit, flush everything at `AgentEnd`.** Re-litigates a settled
  fix, loses the pi-parity mid-turn scroll-into-history, and makes the final flush a
  turn-sized synchronous paint. The auto-scroll tail makes it *less* catastrophic than in
  mid-2026, but it is strictly less faithful than Paths 1-2.

## 7. Task breakdown

1. **S1 — fix:** the Path 1 change in `App::draw` (+ the `live_floor` field comment at
   `app.rs:922-930`, which must stop claiming the floor holds across mid-turn commits and state
   the release rule instead). No new accessor needed: `TranscriptView::pending()`
   (`transcript.rs:409`) is already public.
2. **S2 — re-spec the unit guard** `live_floor_grows_then_holds_during_a_turn_and_resets_when_idle`
   (`app.rs:9221`): the floor must hold across a mid-turn commit **only when nothing is pending
   flush**; add the release case (pending + `raw < floor` ⇒ floor becomes `raw`).
3. **S3 — regression tests in `src/tests/inline_stacking.rs`** (the VT-replay harness; RED at
   HEAD, the §3 numbers are the RED output):
   - `long_turn_response_tail_stays_visible_after_agent_end` — the §3 scenario; assert (a) the
     response tail rows are on the visible grid after the final draw, (b) blank visible rows ≤ the
     idle chrome, (c) editor rule rows == 2 (no stacking), (d) each response line appears exactly
     once across grid + scrollback.
   - `mid_turn_commit_flush_lands_above_the_viewport_visibly` — assert the `MessageEnd` frame
     leaves the committed text on the grid (not only in scrollback).
   - `short_turn_leaves_no_blank_band_above_the_editor` — the 10-line control; blank rows between
     response tail and editor ≤ the indicator-band height.
4. **S4 — pty verification (the M of the effort):** drive a real agentic turn under tmux per
   `REPRO-LOG.md` conventions — one long multi-tool turn and one short turn, on a small (~24-row)
   and a large terminal; capture the visible grid after `AgentEnd`; also a rapid tool-burst run to
   judge the per-commit shrink (Path 1) against the FLICKER bar. Record in `REPRO-LOG.md`.
5. **S5 — bookkeeping:** update the `TUI-090` row in `07-cyrup-tui.md` (mechanism + fix + the
   re-class to `cyrup-original`); re-check **TUI-091** in the same pty session — if reasoning text
   is present in scrollback/screen after this fix, close TUI-091 as a duplicate; link this file
   from both rows.

## 8. Acceptance criteria

- After any turn on an 80×24 terminal, the turn's newest content is visible directly above the
  editor; blank rows between the last content row and the editor ≤ the transient indicator rows.
- No turn's committed content exists *only* in scrollback unless the live tail alone filled the
  screen at its flush moment (the seamless case, §6).
- Every committed line appears exactly once across visible grid + scrollback; no stacked chrome
  (the `inline_stacking` invariants keep holding).
- `cargo test -p cyrup-tui` green, including the re-specified floor guard; pty drive logged.

## 9. Evidence appendix

**Full-screen turn (the reported bug), 80×24, HEAD `7e2e60c`** — final visible grid, replayed from
the real backend byte stream:

```
 0-18  (19 blank rows)
19     ────────────────────────────────────────────────────────────────────────────────
20     (editor, empty)
21     ────────────────────────────────────────────────────────────────────────────────
22     (blank)
23     0.0%/0 • xp                                            anthropic/claude-opus-4-8
```

scrollback: rows 0-31 blank; rows 32-61 `response line 1..30`; then the three tool blocks —
i.e. **one full screen of blank scrollback separates the user from their response**, and the
visible screen is blank but for the idle chrome.

**Viewport height trace** (same run): `7 → 11 → 16 → 21 → 24` by streamed line 20; `24` through
the `MessageEnd` flush and all three tool flushes; `5` after the `AgentEnd` draw — the collapse
fires, but two frames too late to help anything already flushed.

**Short-turn control (10 lines):** 8/10 lines visible, but rows 8-18 blank between the response
tail (row 7) and the editor (row 19) — the vacated-rows band of the secondary defect.
