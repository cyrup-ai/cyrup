---
stage: aug
status: done
updated: 2026-08-29 17:05
aug_against: cyrup HEAD 7913760 · pi v0.84.1 (`packages/tui/src/tui.ts`) · every citation re-verified and every number re-measured on this host, `--release`
---

# Move rendering off the event-fold task

> The run loop folds state and then draws **on the same task**. TUI-092 made each frame
> cheaper (F2) and rarer (F3), but did not change that structure — so a draw that outruns
> the 80 ms spinner tick can still starve the input arm. The code says so itself, at
> [`run.rs:293-301`](../../../crates/cyrup-tui/src/app/run.rs):
>
> > as soon as one `draw_synchronized` costs more than a tick — which is what growing
> > transcripts do — the input arm is never reached again and **the keyboard dies while the
> > screen keeps animating**
>
> **[AUG-2] That sentence is measured, and the cliff is 28× closer than the last round of
> research thought.** It is not reached at ~14,000 rows of prose. It is reached at
> **~500 lines of code in the active turn**, because a streaming turn re-runs
> `syntect` over its *entire* code content on *every single frame* — 174 µs per code line,
> 86 ms at 500 lines, 109% of the spinner tick. §3.0 does not touch that term at all.
>
> **[AUG-3] And the cheapest fix in the whole task was still missing.** There are TWO syntect
> consumers, not one (§0.7). The second — `highlight_code_lines` on tool result bodies
> ([`tool_builtin.rs:58`](../../../crates/cyrup-tui/src/transcript/tool_builtin.rs)) —
> highlights the **entire** output and then renders `total.min(10)` rows. At a 2,000-line
> `read` that is **356 ms per frame of which 99.5% is discarded**, and the fix is one `.take()`
> with **proven byte-identical output** (§0.7 B2/B3). It is a smaller edit than anything else
> here and a larger win than §3.0.
>
> The required fix set is now **§3.0a first** (bound the highlight to what is shown — one
> argument, 187×), then **§3.0b** (resumable/memoised highlighter, 66–3078×), then §3.0
> (wrapped-row cache, removes two full O(n) passes worth ~180 ms at 16k rows), then §3.1.

---

## AUG-3 CHANGE LOG — what moved since the last round

Re-verified against **HEAD `7913760`** (the AUG-2 round read `8f49433`; `crates/cyrup-tui`
moved 31 files between them). Everything below is a correction to this file, not to the code.

| | AUG-2 said | verified now | consequence |
| --- | --- | --- | --- |
| `run_arms.rs` draw sites | `:310,352,358,367,376,422,436,463,475,504,515,521,529,541,550,562,572,608,618,630,640,647` | **all −3**: `:307,349,355,364,373,419,433,460,472,501,512,518,526,538,547,559,569,605,615,627,637,644` | §3.1's call-site list re-numbered below |
| `bump_render_generation()` sites | 39 | **40** | §0.3's argument is unchanged and stronger |
| `walk.rs` final wrap | `:834-838` | **`:837`** | citation only |
| `indexing_slicing` lint | "`lib.rs:46-50`" | **workspace-denied** in root `Cargo.toml`; `lib.rs:50` denies `clippy::string_slice` only | still `.get(..)`, but cite the right place |
| syntect consumers | one (`highlight_lines`) | **two** — `highlight_code_lines` is called from `tool_builtin.rs:58` and `:109` | **new §0.7 / §3.0a** |
| §3.0's height oracle | assumed `rows.len() == wrapped_height(...)` | **two different wrappers; they disagree on 2 of 22 cases** | **new §0.8** — `render_cache.rs` must be re-anchored |
| 30 production `draw_synchronized()` sites | 30 | **30** (3 `crossterm.rs`, 3 `run.rs`, 2 `run_action.rs`, 22 `run_arms.rs`) | unchanged |
| pi `tui.ts` anchors `:343,772,783,806,900` | — | **all exact**; `requestRender(` = 106 in `interactive-mode.ts`; `renderNow()` = 1 (`:815`) | unchanged |
| `arc-swap` at `Cargo.toml:287` | — | **exact**, used by `cyrup-mcp`, `cyrup-resources`, `cyrup-session` | unchanged |
| `ParseState: Clone` | claimed | **verified** — `syntect-5.3.0/src/parsing/parser.rs:57-58` `#[derive(Debug, Clone, Eq, PartialEq)]` | §3.0b is sound |

---

## 0. READ THIS FIRST

Eight findings. §0.1 is inherited. §0.3–§0.5 came from the AUG-2 round. **§0.7 and §0.8 are
new this round; §0.7 adds the cheapest fix in the task and §0.8 removes a correctness trap
from §3.0 that would otherwise present as a rendering regression.**

### 0.1 Do not re-do TUI-092

**[`docs/gap-analysis/bugs/TUI-092-progressive-lockup.md`](../../../docs/gap-analysis/bugs/TUI-092-progressive-lockup.md)
is required reading before touching this.** All eight of its fixes have landed. Every
obvious "make the TUI faster" move is already made:

| | already landed | do not redo |
| --- | --- | --- |
| F1 | scrollback accumulator gated out of production builds | — |
| F2 | `RenderCache` keyed `(generation, width, theme.generation)` | the render cache exists |
| F3 | drain-then-draw on the `events`/`input`/`bash_next` arms | the coalescing exists |
| F4 | `context_usage` reverse scan, zero message clones | — |
| F5 | ratatui `scrolling-regions` on by default | — |
| F6 | `BashExecution::output_lines` bounded at 2000 | — |
| F7 | image protocol memoised per frame | — |
| F8 | by-value event ingest, payloads moved not cloned | — |

**TUI-092 reduced frame cost and frame count. It did not remove the coupling.** Draw still
happens inline on the task that folds events
([`run_action.rs:339`](../../../crates/cyrup-tui/src/app/run_action.rs)).

### 0.2 A cache HIT is O(active turn) — F2's own definition of done is false

TUI-092 §7 property 2 claims *"a frame with unchanged state is O(changed chrome)"*, offering
`cached_render`'s key check as *"the whole proof"*. The cache memoises the markdown + syntect
materialisation; it memoises nothing downstream of it, and
[`TranscriptView::render`](../../../crates/cyrup-tui/src/transcript/cache.rs) pays three costs
on **every** frame, hit or miss — verified verbatim at `cache.rs:228-244`:

```rust
fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
    let width = area.width as usize;
    let (total, lines) = {
        let cache = self.cached_render(width, theme);
        (cache.lines.len(), cache.lines.clone())      // (1) DEEP clone of every line + span
    };
    let inner_h = area.height as usize;
    let max_scroll = total.saturating_sub(inner_h);
    self.scroll_offset = self.scroll_offset.min(max_scroll);
    let scroll = max_scroll.saturating_sub(self.scroll_offset).min(u16::MAX as usize) as u16;
    let para = Paragraph::new(lines)
        .style(theme.base_style())
        .wrap(Wrap { trim: false })                   // (2) re-wraps the WHOLE turn …
        .scroll((scroll, 0));                         // (3) … to paint ≤ area.height rows
    frame.render_widget(para, area);
    crate::osc::inject(frame.buffer_mut(), &self.render_cache.links);
}
```

1. **`cache.lines.clone()` is a deep copy.** `Line<'static>` owns `Vec<Span<'static>>`, each
   `Span` a `Cow::Owned(String)`; one allocation per span, every byte memcpy'd, per frame.
2. **`.wrap(Wrap { trim: false })` re-wraps content that is already wrapped.**
   [`MdRenderer::finish`](../../../crates/cyrup-tui/src/markdown/walk.rs) ends the token walk
   with `self.out.into_iter().flat_map(|l| wrap_line(&l, width))` at `walk.rs:834-838`, and
   [`markdown/mod.rs:98-101`](../../../crates/cyrup-tui/src/markdown/mod.rs) states the
   consequence: *"Rows come back already wrapped to `width` … nothing downstream needs to
   reflow them, and reflowing them at a wider width is exactly the L2/M10 bug."* The comment
   in `cached_render` justifying the second wrap — *"`markdown::render` emits ONE un-wrapped
   `Line` per prose paragraph"* — is **stale**; it describes a renderer that no longer exists.
3. **Nothing is windowed.** The entire `Vec<Line>` goes to `Paragraph`, which wraps all of it
   and discards everything above `scroll`.

Measured, `--release`, 100 × 30 area — [`tmp/perf005-probe`](../../../tmp/perf005-probe),
reproduced this round:

| active-turn rows | A: today | B: drop the deep clone | C: + drop the re-wrap | D: + window the paint |
| --- | --- | --- | --- | --- |
| 20 | **167 µs** | 160 µs | 137 µs | **137 µs** |
| 200 | **1 169 µs** | 1 098 µs | 203 µs | **198 µs** |
| 1 000 | **5 790 µs** | 5 346 µs | 341 µs | **194 µs** |
| 2 000 | **11 424 µs** | 10 507 µs | 535 µs | **193 µs** |
| 4 000 | **23 183 µs** | 21 227 µs | 953 µs | **193 µs** |
| 8 000 | **45 989 µs** | 42 317 µs | 1 869 µs | **195 µs** |
| 16 000 | **90 982 µs** | 83 565 µs | 4 436 µs | **194 µs** |

* **The deep clone is not the problem** (A→B ≈ 4–8%), even though in isolation it costs
  12.1 ms at 16k rows. It is dwarfed by what follows it.
* **The redundant re-wrap is ~95% of the frame** (B→C is 19× at 16k). ratatui's `WordWrapper`
  walks every line before honouring `.scroll()`.
* **Windowing removes the last O(n)** (C→D): **flat at ~194 µs**, independent of turn size.
  That floor is `Buffer::reset()` over 3,000 cells plus the blit.

### 0.3 [AUG-2] But the cache is MISSED on every streaming frame — §0.2 priced the wrong path

`push_assistant_delta` bumps the render generation
([`transcript/stream.rs:55-56`](../../../crates/cyrup-tui/src/transcript/stream.rs)):

```rust
    pub fn push_assistant_delta(&mut self, delta: &str) {
        self.bump_render_generation();
```

There are **39 `bump_render_generation()` call sites** and the streaming delta is one of them.
So **during a streaming turn every frame is a cache miss.** `RenderCache` earns its keep only
on content-quiet repaints (a spinner tick while a tool runs, a keystroke that changes no
content). §0.2's table measures everything *downstream* of `cached_render` — the hit portion.
A streaming frame pays that **plus the whole miss path on top.**

The miss path has two O(n) terms the previous round never priced:

**(i) `wrapped_height` is a second full wrap, not "a second deep clone".**
[`layout.rs:384-391`](../../../crates/cyrup-tui/src/transcript/layout.rs):

```rust
pub(crate) fn wrapped_height(lines: &[Line<'static>], width: usize) -> usize {
    if width == 0 { return lines.len(); }
    Paragraph::new(lines.to_vec())            // deep clone of the whole turn
        .wrap(Wrap { trim: false })
        .line_count(width.min(u16::MAX as usize) as u16)   // …then wrap all of it, to count
}
```

It runs inside `cached_render` on **every miss** (`cache.rs:42`). Measured — column E,
[`tmp/perf005-miss`](../../../tmp/perf005-miss):

| active-turn rows | E: `wrapped_height` per miss | F: `flat_map(wrap_line)` (§3.0 as drafted) | G: move-based (§3.0 done right) |
| --- | --- | --- | --- |
| 200 | **1 048 µs** | 84 µs | **51 µs** |
| 1 000 | **5 434 µs** | 404 µs | **259 µs** |
| 2 000 | **10 951 µs** | 803 µs | **506 µs** |
| 4 000 | **22 106 µs** | 1 684 µs | **1 048 µs** |
| 8 000 | **44 439 µs** | 3 473 µs | **2 039 µs** |
| 16 000 | **88 158 µs** | 7 614 µs | **4 494 µs** |

`wrapped_height` alone costs **88 ms at 16k rows** — the same order as the entire hit-path
frame. The true frame today at 16k rows is therefore ≈ **88 ms (wrapped_height) + 91 ms
(clone + re-wrap + blit) ≈ 180 ms**, before markdown. §3.0 deletes both, because `rows.len()`
*is* the wrapped height.

**(ii) `flat_map(wrap_line)` as §3.0 currently drafts it is a pessimization.**
`wrap_line`'s early return is a **deep clone**
([`layout.rs:51-55`](../../../crates/cyrup-tui/src/transcript/layout.rs)):

```rust
pub(crate) fn wrap_line(line: &Line<'static>, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    if line.width() <= width {
        return vec![line.clone()];        // ← every already-fitting row is CLONED
    }
```

Nearly every row already fits, so the naive `flat_map` clones the whole vector for nothing:
column F, 7.6 ms at 16k rows against 4.5 ms for the move-based form (column G). §3.0 must use
the move-based helper.

### 0.4 [AUG-2] THE CLIFF IS SYNTAX HIGHLIGHTING, AND §3.0 DOES NOT TOUCH IT

The third miss-path term is `lines_with()` itself — a full re-render of the turn's markdown
through `pulldown-cmark` **and `syntect`**. There is no memoisation of results anywhere in
[`markdown/`](../../../crates/cyrup-tui/src/markdown/); the only cache is a
`OnceLock<SyntaxSet>` holding the *grammars*
([`highlight.rs:4-7`](../../../crates/cyrup-tui/src/markdown/highlight.rs)).
[`highlight_inner`](../../../crates/cyrup-tui/src/markdown/highlight.rs) at `highlight.rs:73-102`
builds a **fresh `ParseState` and re-parses every line of every code block, per frame**:

```rust
fn highlight_inner(code: &str, syntax: &SyntaxReference, ss: &SyntaxSet, theme: &UiTheme)
    -> Option<Vec<Line<'static>>> {
    let mut parse = ParseState::new(syntax);          // ← discarded and rebuilt EVERY frame
    let mut out: Vec<Line<'static>> = Vec::new();
    for raw in code.split('\n') {                     // ← the WHOLE block, every frame
        let line_nl = format!("{raw}\n");
        let ops = parse.parse_line(&line_nl, ss).ok()?;
        …
```

Measured with that function reproduced verbatim —
[`tmp/perf005-hl`](../../../tmp/perf005-hl), `--release`:

| code lines in the active turn | `highlight_inner` per frame | µs / code line | vs 16.7 ms (60 Hz) | vs 80 ms spinner tick |
| --- | --- | --- | --- | --- |
| 10 | 1.84 ms | 184 µs | 11% | 2% |
| 50 | 8.52 ms | 170 µs | 51% | 11% |
| 100 | **17.44 ms** | 174 µs | **104%** | 22% |
| 250 | **43.39 ms** | 174 µs | 260% | 54% |
| 500 | **87.10 ms** | 174 µs | 522% | **109%** |
| 1 000 | **171.88 ms** | 172 µs | 1 029% | 215% |
| 2 000 | **343.04 ms** | 172 µs | 2 054% | 429% |

**Read those two right-hand columns.** The 60 Hz budget is blown by **100 lines of code** in
the turn. The 80 ms spinner tick — the documented starvation threshold at `run.rs:293-301` —
is crossed at **~500 lines of code**, not at 14,000 rows of prose. For a *coding* agent that
is an ordinary turn, not a pathological one.

The cost is ~174 µs per code line and flat in that unit, so it scales purely with how much
code is in the turn. It compounds with the deliberate pure-Rust regex choice recorded at
[`Cargo.toml:246-250`](../../../Cargo.toml) — `syntect` with `default-fancy` instead of
oniguruma. **That choice is correct and stays**; the defect is not the engine, it is running
the engine over the entire turn on every frame.

**This term survives §3.0, §3.1, §3.2 and §3.3 untouched.** No amount of frame-cost or
frame-rate work removes an O(turn) markdown re-render from the miss path. It has to be made
incremental, which is §3.0b — and §3.0b is therefore the *first* thing to build.

### 0.5 [AUG-2] A latent scroll bug that §3.0 closes for free

In `TranscriptView::render` (§0.2, line 1) `total` is `cache.lines.len()` — the count of
**logical** lines — but `Paragraph::scroll()` counts **wrapped display rows**. The correct
number is sitting in the same struct, unused on this path: `cache.wrapped_height`, which
`content_height` *does* return (`cache.rs:59`) and which sizes the viewport
([`app/layout.rs:174`](../../../crates/cyrup-tui/src/app/layout.rs)).

When the two diverge, `max_scroll` under-counts and the tail-anchor lands in the middle of
the turn: **the newest streaming text is never shown.** The codebase already knows they can
diverge — [`altscreen/document.rs:125`](../../../crates/cyrup-tui/src/altscreen/document.rs)
branches on exactly that test:

```rust
        if wrapped_height(&lines, width) == lines.len() {
            rows.extend(lines);
            continue;
        }
        for line in &lines { rows.extend(wrap_line(line, width)); }
```

Markdown bodies are pre-wrapped so they usually agree; the rows that reach `lines_with`
through `tool_lines` and `BashExecution::render_lines` carry no such guarantee. §3.0 makes
`rows.len()` the single definition of height and the bug cannot be expressed.

### 0.6 pi DOES decouple render from fold — cyrup ported none of it

pi has a frame scheduler with a hard frame-rate cap. Verified verbatim in
`pi/packages/tui/src/tui.ts`:

```ts
// :343
private static readonly MIN_RENDER_INTERVAL_MS = 16;

// :772-781 — the default path. Sets a flag; paints nothing.
requestRender(force = false): void {
    if (force) { this.resetRenderState(); this.requestImmediateRender(); return; }
    if (this.renderRequested) return;
    this.renderRequested = true;
    process.nextTick(() => this.scheduleRender());
}

// :806-822 — one frame per MIN_RENDER_INTERVAL_MS, however many requests arrived
private scheduleRender(): void {
    if (this.stopped || this.renderTimer || !this.renderRequested) return;
    const elapsed = performance.now() - this.lastRenderAt;
    const delay = Math.max(0, TuiBase.MIN_RENDER_INTERVAL_MS - elapsed);
    this.renderTimer = setTimeout(() => { … this.doRender(); if (this.renderRequested) this.scheduleRender(); }, delay);
}

// :783-796 — input's preempting path: cancels a queued throttled frame outright
private requestImmediateRender(): void {
    this.cancelRenderTimer();
    this.renderRequested = true;
    if (this.immediateRenderScheduled) return;
    this.immediateRenderScheduled = true;
    process.nextTick(() => { … this.cancelRenderTimer(); this.renderRequested = false;
                             this.lastRenderAt = performance.now(); this.doRender(); });
}

// :896-900 — and input takes it, with the reason stated
this.focusedComponent.handleInput(data);
// Keyboard input is latency-sensitive. Avoid the throttled timer path,
// where even setTimeout(0) can take a full 16 ms tick on Windows.
this.requestImmediateRender();
```

**There are THREE modes, not two** — the previous round collapsed the third:

| pi | meaning | cyrup analog |
| --- | --- | --- |
| `requestRender()` | flag; coalesced to ≤1 frame / 16 ms | `frames.request()` |
| `requestRender(true)` | `resetRenderState()` **+** immediate — a **full redraw**: `tui-main-screen.ts:91-99` clears `previousLines`/`previousWidth`/`previousHeight`/`cursorRow`/`maxLinesRendered`, `tui-alt-screen.ts:387-392` clears `previousScreen`/`currentLayout`. 3 call sites in `interactive-mode.ts` | `frames.request_full_redraw()` → `terminal.clear()` before the frame |
| `renderNow()` | `tui.ts:764-770`, synchronous, paints before returning. **1** call site ([`interactive-mode.ts:815`](../../../../pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts)) | keep `draw_synchronized()` |

`interactive-mode.ts` calls `requestRender(` **106 times**; none paints synchronously.

cyrup has **30 production `draw_synchronized()` call sites**, **every one of which paints
immediately**, and no frame cap anywhere in the crate:

```bash
grep -rn 'draw_synchronized()' crates/cyrup-tui/src --include=*.rs \
  | grep -v '/tests/' | grep -v 'fn draw_synchronized' \
  | grep -v '// ' | sed 's/:.*//' | sort | uniq -c
#   3 crates/cyrup-tui/src/app/crossterm.rs
#   3 crates/cyrup-tui/src/app/run.rs
#   2 crates/cyrup-tui/src/app/run_action.rs
#  22 crates/cyrup-tui/src/app/run_arms.rs
grep -rn 'MIN_RENDER\|frame_interval\|render_interval' crates/cyrup-tui/src   # → nothing
```

pi cannot get *parallelism* (§2), but it gets **coalescing across arms and a bounded frame
rate**, which cyrup does not. That is a straight port and it subsumes F3's within-arm drain
with a structural guarantee.

### 0.7 [AUG-3] THERE ARE TWO SYNTECT CONSUMERS, AND THE SECOND ONE THROWS 99.5% OF ITS WORK AWAY

§0.4 found the syntect term. It found **one** of its two call paths. `grep` the crate:

```bash
grep -rn 'highlight_lines\|highlight_code_lines' crates/cyrup-tui/src --include=*.rs \
  | grep -v 'fn highlight'
# markdown/walk.rs:815              highlight_lines(code, lang, self.theme)      <- §0.4's path
# transcript/tool_builtin.rs:58     highlight_code_lines(&replace_tabs(&output), l, theme)
# transcript/tool_builtin.rs:109    highlight_code_lines(&replace_tabs(&display), l, theme)
```

The two paths have **different content dynamics and therefore need different fixes**:

| | A — `highlight_lines` | B — `highlight_code_lines` |
| --- | --- | --- |
| called from | [`MdRenderer::emit_fence_rows`](../../../crates/cyrup-tui/src/markdown/walk.rs) `walk.rs:815` | [`tool_builtin.rs:58`](../../../crates/cyrup-tui/src/transcript/tool_builtin.rs) (`read` result), `:109` (`write`/`edit` preview) |
| content | a fence in the streaming partial — **grows by append** | a settled tool result — **immutable once `run.result` is `Some`** |
| fix | resumable `ParseState` (§3.0b) | bound to what is shown (§3.0a) + content memo (§3.0b) |

**B is the worse of the two, and it is trivially fixable.** `tool_builtin.rs` highlights the
**whole** output and then renders at most ten rows unless the block is expanded:

```rust
// transcript/tool_builtin.rs:57-63 — verbatim
let highlighted =
    lang.and_then(|l| crate::markdown::highlight_code_lines(&replace_tabs(&output), l, theme));
let all = trim_trailing_empty(output.split('\n').collect());
let total = all.len();
let shown = if expanded { total } else { total.min(10) };   // <- TEN
out.push(Line::default());
for (i, l) in all.iter().take(shown).enumerate() {
    out.push(body_line(l, highlighted.as_ref(), i, theme));  // <- indices 0..shown only
}
```

and `body_line` reads that vector with **`.get(idx)`**
([`transcript/layout.rs:371`](../../../crates/cyrup-tui/src/transcript/layout.rs)):

```rust
    match highlighted.and_then(|h| h.get(idx)) {
        Some(l) => l.clone(),
        None => Line::styled(replace_tabs(raw), theme.tool_output_style()),
    }
```

so a vector that covers only `0..shown` is **indistinguishable** from one that covers
`0..total`. syntect is a forward line-at-a-time state machine, so the first `shown` rows of a
full-block highlight are bit-identical to a highlight that stops after `shown` lines —
B3 below asserts exactly that.

Measured, `--release` — [`tmp/perf005-hl2`](../../../tmp/perf005-hl2), `highlight_inner`
reproduced verbatim with a `take(limit)`:

**B2 — a collapsed tool body, per frame**

| output lines | today (highlight all) | bounded to `shown` = 10 | discarded |
| --- | --- | --- | --- |
| 50 | **8.60 ms** | 1 910 µs | 77.8% |
| 100 | **19.08 ms** | 1 884 µs | 90.1% |
| 250 | **44.76 ms** | 1 920 µs | 95.7% |
| 500 | **88.21 ms** | 1 910 µs | 97.8% |
| 1 000 | **180.73 ms** | 1 922 µs | 98.9% |
| 2 000 | **356.88 ms** | 1 907 µs | **99.5%** |

**B3 — equivalence**: the first 10 rows of the full highlight compare `==` to the 10-row
bounded highlight, at a 2,000-line body. The probe asserts it; the change is not an
approximation.

**B1 — and when the block IS expanded, a content memo still wins**, because the body is
immutable: a hit is a `Vec<Line>` clone, not a re-parse.

| lines | re-highlight (today) | memo hit (clone) | speedup |
| --- | --- | --- | --- |
| 50 | 8.80 ms | 42 µs | **207×** |
| 100 | 17.66 ms | 85 µs | **207×** |
| 250 | 43.95 ms | 214 µs | **206×** |
| 500 | 102.70 ms | 441 µs | **233×** |
| 1 000 | 195.10 ms | 1 970 µs | **99×** |
| 2 000 | 457.88 ms | 2 972 µs | **154×** |

A `read` of a 2,000-line file is not a pathological case for a coding agent — it is Tuesday.
And every one of these milliseconds is paid on **every frame of the rest of the turn**, because
`push_assistant_delta` keeps bumping the generation (§0.3) long after the tool settled.

### 0.8 [AUG-3] §3.0 SILENTLY CHANGES THE HEIGHT ORACLE — AND A TEST FILE PINS THE OLD ONE

§3.0 replaces the cached `wrapped_height` with `rows.len()`. Those are **two different
wrapping implementations**:

* `wrapped_height` = `Paragraph::line_count` = **ratatui's `WordWrapper`**
  ([`layout.rs:384-391`](../../../crates/cyrup-tui/src/transcript/layout.rs)).
* `rows` = `wrap_all_owned` → **cyrup's own `wrap_line`**, a port of pi's `wrapSingleLine`
  (`tui/src/utils.ts:857-936`), which right-trims every produced row
  ([`layout.rs:110-127`](../../../crates/cyrup-tui/src/transcript/layout.rs)).

The AUG-2 draft assumed they agree. **They do not.** Both reproduced verbatim and compared over
22 content shapes at width 20 — [`tmp/perf005-wrapeq`](../../../tmp/perf005-wrapeq):

```
case                                    ratatui wrap_all  agree
----------------------------------------------------------------
empty vec / empty lines / short / exactly-width / width+1     ok
trailing space fits / overflows / leading spaces              ok
prose wraps / long unbreakable token / double space run       ok
cjk wide / emoji zwj / multi-span prose / multi-span overflow  ok
blank between prose / tool-output shape (12 rows)             ok
tab (expanded upstream)                       1        2 **NO**
line of only spaces > width                   2        1 **NO**

disagreements: 2 / 22
```

plus a width sweep (10→120) on a real prose paragraph and a pre-wrapped 50-row body at three
widths: **all agree**. So the disagreement is confined to two shapes:

1. **A tab.** Unreachable by construction — [`layout.rs:11-17`](../../../crates/cyrup-tui/src/transcript/layout.rs)
   states it: *"a tab can no longer reach `wrap_line`, because `text_lines` and `normalize_line`
   expand it upstream of every `Line` construction"*, and `replace_tabs` (`layout.rs:272`) is
   applied on the tool path. Verified: the only disagreement is in the probe, which
   deliberately bypasses that normalisation.
2. **A whitespace-only row wider than the pane.** Reachable — from `tool_lines` and
   `BashExecution::render_lines`, which carry no pre-wrapped guarantee. `wrap_line` reports 1
   row, ratatui reports 2.

**The important part is which one is right after §3.0.** Today `wrapped_height` is a
*prediction* of what `Paragraph::render(.wrap(...))` will paint, so it has to model ratatui.
After §3.0 the `.wrap()` is **gone** and the rows in the cache *are* the rows that get blitted —
so `rows.len()` is not an estimate of the height, it **is** the height, and
`wrapped_height` becomes a *wrong* oracle rather than a right one.

That makes the following an obligation, not an option:

**[`transcript/tests/render_cache.rs`](../../../crates/cyrup-tui/src/transcript/tests/render_cache.rs)
must be re-anchored, not deleted** — it asserts the old oracle in **18 places**, including
`assert_fresh` (`:32-40`), which runs after *every step* of a simulated turn:

```rust
fn assert_fresh(view: &mut TranscriptView, theme: &UiTheme, step: &str) {
    let fresh = view.lines(80, theme);
    let fresh_h = wrapped_height(&fresh, 80);              // <- becomes wrap_all_owned(...).len()
    let cache = view.cached_render(80, theme);
    assert_eq!(cache.lines, fresh, …);                     // <- becomes cache.rows vs wrapped fresh
    assert_eq!(cache.wrapped_height, fresh_h, …);          // <- becomes cache.rows.len()
    assert_eq!(view.content_height(80, theme), fresh_h, …);
}
```

The mechanical re-anchor is: `wrapped_height(&fresh, w)` → `wrap_all_owned(fresh.clone(), w).len()`,
`cache.lines` → `cache.rows.as_slice()` compared against `wrap_all_owned(fresh, w)`,
`cache.wrapped_height` → `cache.rows.len()`, and the three `POISON_HEIGHT` pokes
(`:72`, `:86`, `:107`, `:119`, `:274`) become pokes at `cache.rows`. `:355`
(`render_cache.lines = vec![Line::from("SENTINEL-LINE")]`) becomes an `Arc::new(vec![...])`.

Four other files read these members and must be checked in the same change:
[`tests/assembled_render.rs:250,281`](../../../crates/cyrup-tui/src/tests/assembled_render.rs)
(the PROSE-WRAP regression test — it asserts the viewport is **not** sized to `lines.len()`, so
it must keep passing and is the best single proof §3.0 did not regress height),
[`transcript/tests/progressive_commit.rs:35,41`](../../../crates/cyrup-tui/src/transcript/tests/progressive_commit.rs)
(`content_height` stays bounded), [`transcript/tests/osc_hyperlinks.rs:279`](../../../crates/cyrup-tui/src/transcript/tests/osc_hyperlinks.rs)
(`content_height` equal with links on and off — pins §3.0's `links` handling), and
[`tests/render_cache_tick.rs:82`](../../../crates/cyrup-tui/src/tests/render_cache_tick.rs)
(bump-before-draw, which §3.1 also touches).

---

## 1. The coupling

[`app/run_action.rs:280-341`](../../../crates/cyrup-tui/src/app/run_action.rs):

```rust
    pub(crate) async fn on_session_event(&mut self, ctx: &mut RunCtx, ev: …, events: …) -> … {
                    let _arm = ArmGuard::enter("events");            // :293
                    let mut pending = std::collections::VecDeque::from([ev]);
                    while let Some(Some(ev)) = events.next().now_or_never() {   // :306 — F3 drain
                        pending.push_back(ev);
                    }
                    while let Some(ev) = pending.pop_front() {
                        self.ingest_session_event_owned(ev, &ctx.session).await;  // fold
                    }
                    self.draw_synchronized()?;                       // :339 — draw, SAME TASK
```

`draw_synchronized` is [`app/crossterm.rs:87-100`](../../../crates/cyrup-tui/src/app/crossterm.rs).
Every call site is on the run-loop task. The consequence chain:

1. Draw cost grows with the active turn — **§0.3/§0.4, measured: ~180 ms of layout at 16k
   rows, and 87 ms of syntect at 500 code lines, on every streaming frame.**
2. The run loop is `biased;` ([`run.rs:302`](../../../crates/cyrup-tui/src/app/run.rs)) and
   the spinner ticker re-arms every 80 ms
   ([`status_indicator.rs:48`](../../../crates/cyrup-tui/src/status_indicator.rs)).
3. One draw exceeding one tick starves the arms below the ticker; the input arm is hoisted
   above the tickers precisely to mitigate — not remove — this.
4. Meanwhile `Fanout::emit` **awaits** every send
   ([`subscriber.rs:63-72`](../../../crates/cyrup-session-svc/src/subscriber.rs), *"backpressure
   → slows the agent, never drops"*), so a slow draw **throttles the provider stream**.

That last point makes this a throughput task, not a cosmetics task.

**One cost the CPU numbers do not capture at all.** `CrosstermBackend` writes to `io::Stdout`,
and a `write(2)` to a tty **blocks without bound** when the terminal is not draining — flow
control (`Ctrl+S`/XOFF), a slow ssh link, a suspended emulator. That stall is proportional to
nothing and no amount of §3.0/§3.0b removes it. **It is the honest justification for §3.3's
OS thread**, and it is the argument that should decide whether §3.3 gets built.

---

## 2. What pi can and cannot do

pi has one event-loop thread. `Promise.all` is concurrency, not parallelism; renderer, fold
and tool execution contend for that thread by construction.

**But it does not paint inline** (§0.6). Every *"pi cannot do this"* claim applies only to
§3.3 — putting terminal writes on a real thread so a blocking `write(2)` cannot reach the
fold. §3.1 is not a cyrup innovation; it is a port of behaviour cyrup skipped.

---

## 3. Required implementation

Five stages. The order is now **§3.0a, §3.0b, §3.0, §3.1, §3.2, §3.3** — §3.0a is new and goes
first because it is one argument for a 187× win with proven-identical output; §3.0b follows
because §0.4/§0.7 show the syntect term is what actually crosses the spinner tick.

### 3.0a Stop highlighting what is not shown — one `take`, 187×, output-identical

**Do this first. It is the smallest edit in the task and the largest ratio.**

[`transcript/tool_builtin.rs:57-63`](../../../crates/cyrup-tui/src/transcript/tool_builtin.rs)
computes `shown` **after** it has already highlighted everything. Invert that: compute `shown`
first and highlight `0..shown`. Same at the `write`/`edit` preview site, `:105-117`.

Give `highlight_code_lines` a row bound rather than adding a second entry point — there is one
highlighter here for the same reason `wrap_line` is the only wrapper (`layout.rs:44-46`):

```rust
/// … existing doc comment, plus:
///
/// `max_rows` bounds the parse to the rows the caller will actually read. `body_line`
/// (`transcript/layout.rs:369-373`) indexes the returned vector with `.get(idx)` for
/// `idx` in `0..shown`, and syntect is a forward line-at-a-time state machine, so the first
/// `max_rows` rows of a full-block highlight are **identical** to a highlight that stops there.
/// A collapsed `read` block shows `total.min(10)` rows (`tool_builtin.rs:61`) and used to
/// highlight all `total` of them: 356 ms/frame at a 2,000-line file, 99.5% of it discarded,
/// on EVERY frame of the rest of the turn because `push_assistant_delta` keeps bumping the
/// render generation (`transcript/stream.rs:55-56`).
pub(crate) fn highlight_code_lines(
    code: &str,
    lang: &str,
    theme: &UiTheme,
    max_rows: usize,
) -> Option<Vec<Line<'static>>> {
```

and thread it into `highlight_inner`'s loop as `code.split('\n').take(max_rows)`. Both call
sites become:

```rust
        // `shown` BEFORE the highlight, not after it (§3.0a).
        let all = trim_trailing_empty(output.split('\n').collect());
        let total = all.len();
        let shown = if expanded { total } else { total.min(10) };
        let highlighted = lang.and_then(|l| {
            crate::markdown::highlight_code_lines(&replace_tabs(&output), l, theme, shown)
        });
```

Note `replace_tabs(&output)` still runs over the whole body — leave it; it is a linear string
pass, not a parse, and it is what the `body_line` fallback re-applies per row.

`highlight_lines` (the markdown path) takes **no** bound: a fence renders in full, so `shown`
has no meaning there. Pass `usize::MAX` internally.

The **only** behavioural risk is a caller that reads past `shown`. There is none:
`push_read_truncation` (`tool_builtin.rs:70`) and `more_lines_hint` take the *count*, not the
highlighted rows. Confirm with `grep -n 'highlighted' crates/cyrup-tui/src/transcript/tool_builtin.rs`
— every use is `body_line(l, highlighted.as_ref(), i, theme)` inside `.take(shown)`.

### 3.0b Make the highlighter incremental — resumable for the fence, memoised for the body

**This is the largest single win in the task and the AUG-2 round located it in the wrong place.**

syntect is a line-at-a-time state machine and `ParseState` **derives `Clone`** — verified at
`syntect-5.3.0/src/parsing/parser.rs:57-58`, `#[derive(Debug, Clone, Eq, PartialEq)]`. A
streaming code fence only ever *appends* lines, so the parse state after line N is still valid
when line N+1 arrives. Keep it instead of throwing it away.

**Put BOTH caches inside [`markdown/highlight.rs`](../../../crates/cyrup-tui/src/markdown/highlight.rs),
beside the existing `OnceLock<SyntaxSet>` — not in `TranscriptView`.** The AUG-2 draft hung a
`Vec<HighlightCursor>` off `TranscriptView` keyed by fence ordinal. Do not do that, for three
reasons that only surface once you try to write it:

1. `highlight_lines` is called from `MdRenderer::emit_fence_rows` (`walk.rs:815`), deep inside
   the token walk. `TranscriptView` is not reachable from there, and threading
   `&mut Vec<HighlightCursor>` down means changing `render`, `render_with_text_color`,
   `render_message`, `MdRenderer::new` and every one of their test callers — a wide public
   signature change for a private cache.
2. It fixes path A only. Path B (`highlight_code_lines`, §0.7) never goes through
   `MdRenderer` at all, so the tool bodies keep paying full freight.
3. "Fence ordinal within the turn" is not stable: a fence in the *committed* prefix and a fence
   in the streaming partial are rendered by different calls, and `trim_partial_closing_fence`
   can open or close the last fence between frames.

The objection AUG-2 raised against a map — *"the key would have to be the code text, which
changes every delta, so a map only grows"* — is answered by splitting the two dynamics, because
**only one block per frame ever grows**: the last fence of the streaming partial. Everything
else (closed fences, settled tool bodies) is immutable.

```rust
/// Incremental highlighting state, thread-local because the whole render path is single-
/// threaded (`TranscriptView::render` runs on the run-loop task; after §3.2 the fold still owns
/// materialisation) and a thread-local costs no synchronisation on the hot path.
///
/// `try_borrow_mut` rather than `borrow_mut`: `RefCell` panics on re-entrancy and the workspace
/// denies `clippy::panic`. The mermaid fallback re-enters `emit_fence_rows` (`walk.rs:756`), so
/// the defensive form is not theoretical — and its fallback is exactly today's uncached
/// behaviour, which is always correct.
thread_local! {
    static HL: std::cell::RefCell<HighlightState> = std::cell::RefCell::new(HighlightState::new());
}

struct HighlightState {
    /// THE growing block: the last fence of the streaming partial. Exactly one, because only
    /// the tail fence appends — every earlier fence in the same turn is closed and immutable.
    cursor: Option<HighlightCursor>,
    /// The immutable blocks: closed fences and settled tool bodies. Bounded, evicted by
    /// insertion order, so it cannot grow the way a text-keyed map would.
    memo: std::collections::VecDeque<(MemoKey, std::rc::Rc<Vec<Line<'static>>>)>,
}

/// 16 entries covers a turn's worth of closed fences plus the visible tool blocks; past that,
/// the evicted entry rebuilds at exactly today's cost.
const MEMO_CAP: usize = 16;

/// Invalidation key. `hash` of the code text because the body is immutable once keyed; `lang`
/// because a fence's info string can change while it streams (```` ```ru ```` → ```` ```rust ````);
/// `theme_generation` because the emitted spans carry RESOLVED colours, exactly as `RenderCache`
/// keys on it (`transcript/mod.rs:326`); `max_rows` because §3.0a makes the row bound part of
/// the result. Hash-only would be a collision bet — store `len` too and compare it, which costs
/// one `usize` and removes the bet.
#[derive(PartialEq, Eq)]
struct MemoKey { hash: u64, len: usize, lang: String, theme_generation: u64, max_rows: usize }

/// A resumable highlight of ONE growing code block.
struct HighlightCursor {
    lang: String,
    theme_generation: u64,
    /// The exact text already consumed, so the prefix test below is a comparison, not a guess.
    consumed_text: String,
    /// The syntect state after `consumed_text`. THE reason this type exists.
    parse: ParseState,
    rows: Vec<Line<'static>>,
}
```

The entry point replaces the body of `highlight_inner` and nothing above it:

```rust
fn highlight_inner(code: &str, lang: &str, syntax: &SyntaxReference, ss: &SyntaxSet,
                   theme: &UiTheme, max_rows: usize) -> Option<Vec<Line<'static>>> {
    HL.with(|hl| {
        let Ok(mut st) = hl.try_borrow_mut() else {
            return highlight_uncached(code, syntax, ss, theme, max_rows);  // today's loop, verbatim
        };
        // 1. Immutable hit? Clone the rows — 99-233× cheaper than re-parsing (§0.7 B1).
        if let Some(rows) = st.memo_get(code, lang, theme.generation, max_rows) {
            return Some(rows.as_ref().clone());
        }
        // 2. A strict line-prefix EXTENSION of what the cursor already consumed? Parse the tail.
        //    This is what makes reuse SOUND: a delta appends, but a re-render after an edit, a
        //    `/clear`, or a committed-then-restreamed turn does not. Anything that is not a
        //    strict prefix extension rebuilds from scratch — correctness first, and the rebuild
        //    is exactly today's cost, so the worst case is a wash.
        st.resume_or_rebuild(code, lang, theme, syntax, ss, max_rows)
    })
}
```

**Two rules the implementation must not violate.**

**(a) Do not consume the last line of an OPEN fence into the cursor.** The live path runs
`trim_partial_closing_fence` ([`cache.rs:163`](../../../crates/cyrup-tui/src/transcript/cache.rs)),
so the final line of a streaming block can be a partial token that changes on the next delta.
Parse `code` up to its last `\n` into the cursor and re-parse the tail each frame. One line is
~130 µs, flat, and it is the difference between correct and subtly-wrong colouring.

**(b) Only complete blocks enter the memo.** A block is memo-eligible when its text is not a
prefix of anything currently streaming — in practice, whenever `highlight_code_lines` is the
caller (settled tool bodies, always) and when a fence's text has stopped growing.

**Do not lose the fallbacks.** [`highlight_lines`](../../../crates/cyrup-tui/src/markdown/highlight.rs)
at `:12-29` returns `flat()` for an empty/unknown language token or any syntect fault, and
[`highlight_code_lines`](../../../crates/cyrup-tui/src/markdown/highlight.rs) at `:42-69`
returns `None` for the same and strips the 2-space gutter. Both must keep behaving exactly as
they do — the caches sit *inside* `highlight_inner`'s position, below those gates, so an
unknown language never reaches them.

Measured, `--release` — [`tmp/perf005-hl2`](../../../tmp/perf005-hl2) B4, the cost of the frame
a streaming delta produces, with `ParseState::clone()` included (production keeps the state and
does not pay it, so these are over-estimates):

| code lines already in the fence | today (re-highlight all) | §3.0b (parse the new line) | speedup |
| --- | --- | --- | --- |
| 50 | 9.13 ms | 137 µs | **66×** |
| 100 | 18.03 ms | 116 µs | **155×** |
| 250 | 45.41 ms | 116 µs | **393×** |
| 500 | 91.07 ms | 134 µs | **682×** |
| 1 000 | 176.89 ms | 131 µs | **1 346×** |
| 2 000 | 354.89 ms | 115 µs | **3 078×** |

The §3.0b column is **flat** — one line's parse, independent of block size.

<!-- AUG-2's cursor sketch, superseded by the design above; kept because its field-by-field
     rationale is still the right rationale. -->
<details><summary>AUG-2's original <code>HighlightCursor</code> sketch (superseded)</summary>

```rust
/// A resumable highlight of ONE code block. `highlight_inner` used to build a fresh
/// `ParseState` and re-parse every line on every call, and `cached_render` misses on every
/// streaming delta (`transcript/stream.rs:55-56`) — so a turn holding 500 lines of code paid
/// 87 ms of syntect per frame, 109% of the 80 ms spinner tick (`status_indicator.rs:48`).
///
/// syntect is a line-at-a-time state machine and `ParseState: Clone`, so the state after line
/// N stays valid for line N+1. A streaming fence only appends; parse the tail, keep the rows.
pub(super) struct HighlightCursor {
    /// Invalidation key. `lang` because the fence's info string can change while it streams
    /// (```` ```ru ```` → ```` ```rust ````); `theme_generation` because the emitted spans
    /// carry resolved colours, exactly as `RenderCache` keys on it (`transcript/mod.rs:326`).
    lang: String,
    theme_generation: u64,
    /// Lines already turned into `rows`. The resume point.
    consumed: usize,
    /// The syntect state after `consumed` lines. THE reason this type exists.
    parse: ParseState,
    rows: Vec<Line<'static>>,
}

impl HighlightCursor {
    /// Rows for `code`, parsing only what is new since the last call.
    ///
    /// The prefix check is what makes reuse SOUND: a delta appends, but a re-render after an
    /// edit, a `/clear`, or a committed-then-restreamed turn does not. Anything that is not a
    /// strict line-prefix extension rebuilds from scratch — correctness first, and the rebuild
    /// is exactly today's cost, so the worst case is a wash.
    fn rows_for(&mut self, code: &str, lang: &str, theme: &UiTheme, ss: &SyntaxSet,
                syntax: &syntect::parsing::SyntaxReference) -> &[Line<'static>] {
        let reusable = self.lang == lang
            && self.theme_generation == theme.generation
            && self.consumed <= code.split('\n').count()
            && code.split('\n').take(self.consumed).eq(self.prefix_lines());
        if !reusable {
            self.lang = lang.to_string();
            self.theme_generation = theme.generation;
            self.consumed = 0;
            self.parse = ParseState::new(syntax);
            self.rows.clear();
        }
        for raw in code.split('\n').skip(self.consumed) {
            // Body identical to today's `highlight_inner` loop (highlight.rs:81-100) — it is
            // moved here, not rewritten, so the span/scope semantics T5 pins are untouched.
            …
            self.rows.push(Line::from(spans));
            self.consumed += 1;
        }
        &self.rows
    }
}
```

**Where the cursor lives.** Not in a global map — the key would have to be the code text,
which changes every delta, so a map only grows. Hang it off the render path that already has
a stable identity for the block: `TranscriptView` owns the active turn, so the cursor belongs
beside `render_cache` in [`transcript/mod.rs:323-333`](../../../crates/cyrup-tui/src/transcript/mod.rs)
as `Vec<HighlightCursor>` indexed by the block's ordinal within the turn, reset by
`commit_assistant`/`discard_streaming`. Ordinal is stable during a stream because fences only
ever appear in order and only the last one grows.

</details>

### 3.0 Make a frame O(visible), not O(active turn)

**Wrap once, into the cache. Share the rows. Paint only what fits.**

[`transcript/mod.rs:323-333`](../../../crates/cyrup-tui/src/transcript/mod.rs) — the cache
holds **already-wrapped display rows** behind an `Arc`, and `wrapped_height` disappears as a
separate field because it *is* `rows.len()`:

```rust
struct RenderCache {
    generation: u64,
    width: usize,
    theme_generation: u64,
    /// ALREADY-WRAPPED display rows, one `Line` per screen row, shared not copied.
    /// `Arc` because every consumer wants the whole vector and none mutates it:
    /// `TranscriptView::render` (per frame), `content_height` (per frame) and — after §3.2 —
    /// the published `FrameState`. `Arc::clone` is 11 ns against 12.1 ms for the deep copy it
    /// replaces at a 16k-row turn.
    ///
    /// Replaces the separate `wrapped_height: usize`: that field was recomputed per MISS by
    /// `wrapped_height()`, a `lines.to_vec()` plus a full `Paragraph::line_count` wrap costing
    /// 88 ms at 16k rows (§0.3). `rows.len()` is the same quantity, exactly, for free.
    rows: Arc<Vec<Line<'static>>>,
    links: crate::osc::LinkSink,
}
```

[`transcript/cache.rs:28-56`](../../../crates/cyrup-tui/src/transcript/cache.rs) —
`cached_render` wraps on the miss path, where the markdown pass already is, **moving every row
that already fits**:

```rust
        if stale {
            let links = crate::osc::LinkSink::new();
            let lines = self.lines_with(width, theme, Some(&links));
            self.render_cache = RenderCache {
                generation: self.render_generation,
                width,
                theme_generation: theme.generation,
                rows: Arc::new(wrap_all_owned(lines, width.max(1))),
                links,
            };
        }
```

```rust
/// Wrap a materialised turn into display rows, MOVING every row that already fits.
///
/// Not `lines.into_iter().flat_map(|l| wrap_line(&l, w))`: `wrap_line`'s early return is
/// `vec![line.clone()]` (`layout.rs:53-55`), a deep clone of a `Line` whose spans each own a
/// `String` — and almost every row takes that branch, because `MdRenderer::finish` already
/// wrapped the markdown to `width` (`markdown/walk.rs:834-838`). Measured at a 16k-row turn:
/// 7.6 ms for the `flat_map`, 4.5 ms for this (§0.3, columns F and G).
///
/// The `wrap_line` call is still needed for the rows the inner wrap cannot bound — deeply
/// nested quoted lists at a narrow pane (`walk.rs:829-833`) — and for the rows that never went
/// through markdown at all: `tool_lines` and `BashExecution::render_lines` carry no
/// pre-wrapped guarantee. That is why the wrap MOVES here rather than being deleted.
pub(crate) fn wrap_all_owned(lines: Vec<Line<'static>>, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut out = Vec::with_capacity(lines.len());
    for line in lines {
        if line.width() <= width {
            out.push(line);                       // MOVE — no allocation, no memcpy
        } else {
            out.extend(wrap_line(&line, width));
        }
    }
    out
}
```

[`transcript/cache.rs:228-249`](../../../crates/cyrup-tui/src/transcript/cache.rs) — render
becomes a refcount bump, a slice, and a bounded blit:

```rust
    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
        let width = area.width as usize;
        let inner_h = area.height as usize;
        // 11 ns. The rows outlive the borrow, so nothing holds `&mut self` across the paint.
        let rows = Arc::clone(&self.cached_render(width, theme).rows);
        let total = rows.len();
        let max_scroll = total.saturating_sub(inner_h);
        self.scroll_offset = self.scroll_offset.min(max_scroll);
        // `total` is now WRAPPED rows, which is what the scroll unit always was — closing the
        // §0.5 mismatch where `cache.lines.len()` (logical) was compared against a wrapped
        // scroll offset and the tail-anchor missed the newest text.
        //
        // Every row is pre-wrapped, so this index is an exact screen row: `Paragraph` needs
        // neither `.wrap()` (the rows already fit) nor `.scroll()` (we sliced instead), and the
        // deep copy below is bounded by the VIEWPORT, not by the turn.
        let first = max_scroll.saturating_sub(self.scroll_offset);
        let window: Vec<Line<'static>> =
            rows.get(first..(first + inner_h).min(total)).unwrap_or(&[]).to_vec();
        frame.render_widget(Paragraph::new(window).style(theme.base_style()), area);
        crate::osc::inject(frame.buffer_mut(), &self.render_cache.links);
    }
```

`.get(..)` rather than `[..]` — `clippy::indexing_slicing` is **workspace**-denied
([`Cargo.toml` `[workspace.lints.clippy]`](../../../Cargo.toml)) and `clippy::string_slice` is
denied crate-locally at [`lib.rs:50`](../../../crates/cyrup-tui/src/lib.rs). Both fire **only
under `cargo clippy`**, never `cargo build`/`cargo test`.

`content_height` becomes `self.cached_render(width, theme).rows.len()`.

**Do the same on the commit path** ([`app/draw.rs:248-292`](../../../crates/cyrup-tui/src/app/draw.rs)),
which pays the identical triple today — `entry_lines` → `wrapped_height(&lines, width)` (a
`to_vec()` + a full wrap, `:282`) → `Paragraph::new(lines).wrap(…)` inside `insert_before`
(`:285-287`), a **third** wrap. Build `rows` with `wrap_all_owned`, use `rows.len()` as the
`insert_before` height, and drop `.wrap(Wrap { trim: false })` from the `Paragraph`. Note the
`#[cfg(any(test, feature = "scrollback-accumulator"))]` line at `:274` clones again; leave it,
it is already gated out of production by TUI-092 F1.

This is also where §3.0 becomes *safer* than it looks: today `insert_before(height, …)`
reserves a height **predicted** by one wrapper and then paints with another
(`Paragraph::wrap`). After the change the reserved height and the painted rows are the same
`Vec`, so the PROSE-WRAP clipping class (R-ARCH-TUI-003/-005) becomes unrepresentable rather
than merely fixed.

Four hazards, each of which will look like a rendering regression if missed:

* **[AUG-3] `rows.len()` is NOT `wrapped_height(...)` — and that is correct, but the tests
  disagree.** §0.8 measures the two wrappers over 22 shapes: 20 agree, 2 do not (a tab, which
  `replace_tabs`/`normalize_line` make unreachable, and a whitespace-only row wider than the
  pane, which `tool_lines`/`BashExecution::render_lines` CAN produce). After §3.0 the
  `Paragraph` carries no `.wrap()`, so `rows` *are* the painted rows and `rows.len()` is the
  height by definition; `wrapped_height` becomes the wrong oracle. **Re-anchor
  `transcript/tests/render_cache.rs` (18 assertions, listed in §0.8) rather than deleting it**,
  and keep `tests/assembled_render.rs:250,281` green — that is the PROSE-WRAP regression test
  and it is the best single proof §3.0 did not regress height.
* **`content_height` must keep meaning wrapped rows.** It feeds `live_region_height` →
  `region_constraints` → the inline viewport height
  ([`app/layout.rs:174`](../../../crates/cyrup-tui/src/app/layout.rs),
  [`app/draw.rs:56-140`](../../../crates/cyrup-tui/src/app/draw.rs)). `rows.len()` is the same
  quantity, now exact rather than re-measured — but a `lines.len()` left anywhere would
  under-size the viewport and reintroduce the PROSE-WRAP truncation `wrapped_height` was
  written for. `transcript/tests/progressive_commit.rs:35,41` and
  `transcript/tests/osc_hyperlinks.rs:279` both read `content_height` and must stay green.
* **`osc::inject` alignment.** Its doc requires the marked cells to exist before injection and
  `Buffer::diff_iter` to stay column-aligned. Slicing changes *which* rows reach the buffer,
  not their cell layout, so the contract holds — but injection must still run **after**
  `render_widget`, exactly as it does now.
* **`wrapped_height` keeps five other DIRECT production callers** — verified with
  `grep -rn 'wrapped_height(' crates/cyrup-tui/src --include=*.rs | grep -v 'fn wrapped_height'`:
  `selector/mod.rs:208` (inside `title_wrapped_height`), `login_dialog.rs:460` **and** `:470`,
  `altscreen/document.rs:125`, `chrome.rs:159`. (AUG-2 listed `text_input.rs:604` and
  `extension_editor.rs:205`; those call `title_wrapped_height`, not `wrapped_height` — they are
  *indirect* and need no edit.) Keep the free function crate-public; only the two transcript
  call sites (`cache.rs:42`, `app/draw.rs:282`) go away.

### 3.1 Port pi's frame scheduler — request, don't paint

One frame per `MIN_RENDER_INTERVAL`, input preempts, a full-redraw mode, and a `renderNow`
escape hatch for paths that must have pixels before they return.

```rust
/// pi `TuiBase.MIN_RENDER_INTERVAL_MS` (`packages/tui/src/tui.ts:343`) — a 62.5 Hz cap.
pub(crate) const MIN_RENDER_INTERVAL: Duration = Duration::from_millis(16);

/// pi's `renderRequested` / `renderTimer` / `lastRenderAt` triple (`tui.ts:772-822`), owned by
/// the run loop. A request is a FLAG, never a paint: N arms firing inside one interval produce
/// one frame — F3's guarantee upheld ACROSS arms instead of inside each one.
pub(crate) struct FrameScheduler {
    requested: bool,
    /// pi's `requestImmediateRender` (`tui.ts:783-796`): a keystroke must not wait out the
    /// throttle. Set only by the input arm.
    force: bool,
    /// pi's `requestRender(true)` → `resetRenderState()` (`tui.ts:773-777`,
    /// `tui-main-screen.ts:91-99`). Upstream drops its whole line-diff state so the next frame
    /// repaints from scratch; cyrup's equivalent is `terminal.clear()` before the draw, since
    /// ratatui's diff lives in the `Terminal`'s back buffer.
    full: bool,
    last: Instant,
}

impl FrameScheduler {
    pub(crate) fn request(&mut self) { self.requested = true; }
    pub(crate) fn request_immediate(&mut self) { self.requested = true; self.force = true; }
    pub(crate) fn request_full_redraw(&mut self) {
        self.requested = true; self.force = true; self.full = true;
    }
    /// Is a frame owed RIGHT NOW?
    pub(crate) fn due(&self) -> bool {
        self.requested && (self.force || self.last.elapsed() >= MIN_RENDER_INTERVAL)
    }
    /// How long until one is — `None` when nothing is pending, which the run loop expresses as
    /// the `pending()` arm every optional arm there already uses.
    pub(crate) fn due_in(&self) -> Option<Duration> {
        if !self.requested { return None; }
        if self.force { return Some(Duration::ZERO); }
        Some(MIN_RENDER_INTERVAL.saturating_sub(self.last.elapsed()))
    }
    /// Consume the request. Returns whether this frame must repaint from scratch.
    pub(crate) fn taken(&mut self) -> bool {
        let full = self.full;
        self.requested = false; self.force = false; self.full = false;
        self.last = Instant::now();
        full
    }
}
```

Wiring, in [`app/run.rs`](../../../crates/cyrup-tui/src/app/run.rs):

```rust
        'run: loop {
            self.drain_over_budget_arm();
            // The ONE production frame site. At the top of the body, so it batches every arm
            // that fired on the previous iteration regardless of which — pi's `scheduleRender`
            // callback, in the one place cyrup has to put it.
            if self.frames.due() {
                let _frame = ArmGuard::enter("frame");
                if self.frames.taken() { let _ = self.terminal.clear(); }
                self.draw_synchronized()?;
            }
            // Wakes the loop for a frame that is pending but not yet due. Never resolves when
            // nothing is requested, so an idle session costs no wakeups — the same shape as the
            // `overlay_ticked` / `alt_timer` arms above it.
            let frame_due = async {
                match self.frames.due_in() {
                    Some(d) => tokio::time::sleep(d).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                biased;
                _ = cancel.cancelled() => break,
                maybe_in = input.next() => …,
                swapped = session_swapped => …,
                () = frame_due => {}   // wake only; the draw is at the top of the body
                …
            }
```

The `frame_due` arm carries **no body** deliberately: putting the draw in an arm would make
frames starvable by any hotter arm above it under `biased;` — the exact failure this task
exists to remove.

Then convert the 30 call sites:

* **25 become `self.frames.request();`** — pi's `requestRender()`: all 22 in `run_arms.rs`
  (**re-verified at HEAD `7913760`, every AUG-2 number was 3 too high**)
  `:307,349,355,364,373,419,433,460,472,501,512,518,526,538,547,559,569,605,615,627,637,644`,
  the events arm at [`run_action.rs:339`](../../../crates/cyrup-tui/src/app/run_action.rs),
  and `on_altscreen_tick` at [`run.rs:453`](../../../crates/cyrup-tui/src/app/run.rs) (an
  ordinary arm despite living in `run.rs`). Note `run_arms.rs:637` is a tail-position
  `self.draw_synchronized()` — it is `on_share_msg`, whose return type is
  `Result<(), TuiError>` — so it becomes `self.frames.request(); Ok(())`.

  Regenerate the list rather than trusting it; the file moves:

  ```bash
  grep -n 'draw_synchronized' crates/cyrup-tui/src/app/run_arms.rs
  ```
* **The input arm** ([`run_action.rs:268`](../../../crates/cyrup-tui/src/app/run_action.rs))
  becomes `self.frames.request_immediate();` — pi's `requestImmediateRender()` at
  `tui.ts:896-900`, for the reason pi states in that comment.
* **Five stay synchronous `draw_synchronized()`** — pi's `renderNow()` — because control
  leaves the loop immediately afterwards and a deferred frame would never land: the seed frame
  at [`run.rs:120`](../../../crates/cyrup-tui/src/app/run.rs); the post-`stop_fullscreen`
  frame on the exit path at [`run.rs:419`](../../../crates/cyrup-tui/src/app/run.rs); and the
  three terminal-handed-back redraws in
  [`crossterm.rs:136`](../../../crates/cyrup-tui/src/app/crossterm.rs) (`suspend`, after `fg`
  — already paired with its own `terminal.clear()` at `:135`),
  [`:150`](../../../crates/cyrup-tui/src/app/crossterm.rs) (`open_external_editor`) and
  [`:170`](../../../crates/cyrup-tui/src/app/crossterm.rs) (`open_external_editor_for_selector`).
* **The exit path must flush a pending frame** before `drain_and_restore`, or the last state
  change before a quit is never drawn. `run.rs:404-406`'s `if self.state.should_quit { break; }`
  leaves the loop without passing the top-of-body site again.
* `ArmGuard::enter` ([`app/input_reader.rs:119-142`](../../../crates/cyrup-tui/src/app/input_reader.rs))
  keeps bracketing arm bodies; the `"frame"` guard above lets the wedge detector name a slow
  paint.

**Four structural guards read the source and will fail — re-anchor them, do not delete them.**
They use `include_str!` on the very files being edited and count `draw_synchronized()`
literally. All four verified present at HEAD:

| file | what it pins | becomes |
| --- | --- | --- |
| [`run_loop_draw_coalescing.rs`](../../../crates/cyrup-tui/src/tests/run_loop_draw_coalescing.rs) | `arm.matches("draw_synchronized()").count() == 1` per arm (`:111`, `:158`); ordering `ArmGuard` < `now_or_never` drain < draw (`:127`, `:170`) | count `frames.request()` instead; ordering becomes guard < drain < request |
| [`run_loop_input_priority.rs`](../../../crates/cyrup-tui/src/tests/run_loop_input_priority.rs) | the input arm sits above every ticker in the `biased;` block (`:53-70`) | unchanged, but the rationale comment it quotes moves (§3.5) |
| [`render_cache_tick.rs`](../../../crates/cyrup-tui/src/tests/render_cache_tick.rs) | `bump_render_tick()` precedes the draw in `on_spinner_tick` / `on_elapsed_tick` (`:76-79`, `:102-105`) | bump-before-**request**; the `:82` message needs rewording too |
| [`resize_viewport_failure.rs`](../../../crates/cyrup-tui/src/tests/resize_viewport_failure.rs) | draw behaviour on a failed viewport resize — it drives a **fault-injecting backend**, not a source scan, so it is likely unaffected | verify it still exercises a real paint; if its trigger is one of the five synchronous survivors, nothing changes |

All three source-scanning guards `include_str!` `../app/run.rs` (+ `run_arms.rs`,
`run_action.rs`, `transcript/cache.rs`), so **every one of them breaks the moment the loop is
re-split**. `arm_body()` **panics** with *"if the loop was re-split, re-anchor this guard rather
than reading to EOF"* when an anchor stops matching — that message is the instruction; follow
it.

### 3.2 Split the state the renderer reads from the state the loop mutates

The renderer needs a consistent snapshot, not a lock on live `AppState`. Publish with
[`arc_swap::ArcSwap`](../../../Cargo.toml) — already a workspace dependency at
`Cargo.toml:287` and already used by `cyrup-mcp`, `cyrup-resources` and `cyrup-session` — so
the fold does `store(Arc::new(next))` (wait-free) and the renderer does `load()`. A frame the
renderer misses is a frame it did not need to draw.

```rust
/// What one frame needs, produced by the fold and consumed by the render thread. Owns its data
/// outright: the render thread must never hold a borrow into `AppState`, or the fold blocks on
/// it and the decoupling buys nothing.
pub(crate) struct FrameState {
    /// §3.0's cache, shared. This is why §3.0 comes first: publishing is an `Arc::clone`.
    rows: Arc<Vec<Line<'static>>>,
    links: crate::osc::LinkSink,
    /// The first visible row — resolved by the fold, because `scroll_offset` is fold state.
    first_row: usize,
    /// Materialised chrome, all viewport-bounded: band, editor, selector/loader slot,
    /// completion popup, extension header/footer/widgets, image strip, overlays.
    chrome: Chrome,
    /// `[header, msg, pending, band, images, wabove, slot, popup, wbelow, footer]` — the exact
    /// output of `region_constraints` (`app/layout.rs:48`), resolved once by the fold so the two
    /// sides cannot disagree on row counts.
    regions: [u16; 10],
    geometry: Geometry,      // term_w, term_h, viewport_height, live_floor
    cursor: Option<Position>,
    theme: Arc<UiTheme>,
    /// The `insert_before` payload, pre-wrapped by §3.0, or empty on a non-commit frame.
    commits: Arc<Vec<Line<'static>>>,
}
```

**The blocker to plan around: cyrup's render path is `&mut`.**
[`render(frame, state: &mut AppState)`](../../../crates/cyrup-tui/src/app/render.rs) at
`render.rs:4`, and [`Component::render`](../../../crates/cyrup-tui/src/component.rs) at
`component.rs:19` is `fn render(&mut self, …)`. The transcript mutates `scroll_offset` and its
cache, the editor its wrap state, the selector and every overlay as they paint. So the render
thread **cannot** be handed an `&AppState`, and `FrameState` cannot be a view — it must be the
*materialised output* of those components.

Give each chrome component a
`fn lines(&mut self, width: u16, theme: &UiTheme) -> Vec<Line<'static>>` that its existing
`render` then blits, and have the fold call `lines()` while the render thread blits. This is
affordable **only** because every one of those components is viewport-bounded — the editor
caps at `max(5, rows * 3 / 10)` ([`app/layout.rs:25`](../../../crates/cyrup-tui/src/app/layout.rs)),
the band is 2 rows, the footer 1 — whereas the transcript is not, which is why the transcript
rides the `Arc` from §3.0 instead.

Two carries that are easy to lose:

* **The selector caret.** [`app/render.rs`](../../../crates/cyrup-tui/src/app/render.rs)
  derives it by scanning the *rendered buffer*
  (`crate::selector::caret_cell(frame.buffer_mut(), slot_area)`). Produce it directly from the
  selector's `lines()` instead; a buffer scan on the render thread re-introduces a render-side
  computation the publish was supposed to have settled.
* **`publish_extension_readbacks`** runs at the top of `App::draw` (`draw.rs:56-66`) so a guest
  reading the editor buffer or theme name sees what the frame is about to show. It is fold
  state, not paint: it moves to the publish, not to the thread.

### 3.3 Run the terminal writes on a dedicated OS thread

A `std::thread` (not a tokio task — it does blocking terminal I/O and must not occupy a
runtime worker) that wakes on a publish notification or a ~60 Hz timer, `load()`s the current
`Arc<FrameState>`, and draws. Terminal ownership moves entirely to this thread.

**Build this only if §5.4's exit criterion says so.** The justification is not the CPU cliff —
§3.0b and §3.0 close that — it is the unbounded blocking `write(2)` named at the end of §1.

`self.terminal` has **21 uses across 8 files**, the tractable half. The untracked half is
everything writing escapes straight to `io::stdout()`, which must now be ordered against
frames:

| writer | site |
| --- | --- |
| the frame itself (`insert_before`, `resize_viewport` rebuild, `terminal.draw`) | [`app/draw.rs:56-292`](../../../crates/cyrup-tui/src/app/draw.rs) |
| the BSU/ESU bracket + `flush_terminal_progress` | [`app/crossterm.rs:87-100`](../../../crates/cyrup-tui/src/app/crossterm.rs), [`app/shell.rs:97`](../../../crates/cyrup-tui/src/app/shell.rs) |
| OSC 0 window title | [`app/input_reader.rs:13`](../../../crates/cyrup-tui/src/app/input_reader.rs) |
| OSC 9;4 progress + keepalive | [`terminal_progress.rs:133`](../../../crates/cyrup-tui/src/terminal_progress.rs) |
| raw-mode / bracketed-paste / Kitty toggles, `terminal.clear()` | [`app/crossterm.rs:50,129,219`](../../../crates/cyrup-tui/src/app/crossterm.rs) |
| `restore` / `drain_and_restore` | [`app/shell.rs:61,83`](../../../crates/cyrup-tui/src/app/shell.rs), [`drain.rs:144`](../../../crates/cyrup-tui/src/drain.rs) |
| alternate-screen enter/leave | [`altscreen/terminal.rs:163,214`](../../../crates/cyrup-tui/src/altscreen/terminal.rs) |
| the panic hook | [`panic_hook.rs:56`](../../../crates/cyrup-tui/src/panic_hook.rs) |
| the escape hatch's hard exit | [`app/input_reader.rs:194`](../../../crates/cyrup-tui/src/app/input_reader.rs) |
| OSC 52 clipboard | [`clipboard.rs:363`](../../../crates/cyrup-tui/src/clipboard.rs) |

Sequencing hazards that must be handled explicitly:

- **Alternate-screen enter/leave, raw-mode toggles, terminal restore** must be ordered against
  frames — today trivially, because everything is one task. See
  [`app/crossterm.rs:110-112,200`](../../../crates/cyrup-tui/src/app/crossterm.rs) (the
  by-design block announced to the wedge detector via `TerminalReleased::enter()`) and
  [`app/backend.rs`](../../../crates/cyrup-tui/src/app/backend.rs)'s `append_lines` anchoring,
  which depends on knowing exactly what was drawn last. Model them as **commands on the render
  thread's own queue**, not as calls that race it; the anchor
  ([`InlineBackend::anchor`](../../../crates/cyrup-tui/src/app/backend.rs), TUI-093) is
  render-thread-private state and a second writer invalidates it.
- **`TERMINAL_RELEASED` becomes load-bearing for the renderer too.** The flag at
  [`app/input_reader.rs:70-93`](../../../crates/cyrup-tui/src/app/input_reader.rs) marks the
  windows where the terminal belongs to a child (`suspend`, `edit_in_external_editor`). The
  render thread **must** consult it and paint nothing while it is set, or a frame lands in the
  middle of the user's `$EDITOR`.
- **`drain_and_restore` at shutdown** must stop the render thread and take the terminal back
  before restoring — the ordering the existing `stop_fullscreen(false)` + final-draw sequence
  at [`run.rs:405-420`](../../../crates/cyrup-tui/src/app/run.rs) gets for free today.
- **Panic safety.** [`panic_hook.rs`](../../../crates/cyrup-tui/src/panic_hook.rs) is
  process-global (`std::panic::set_hook`) so it already fires from any thread, and it closes
  the synchronized-update bracket **first** (`panic_hook.rs:56-70`) — exactly what a render
  thread dying mid-frame needs. It stays correct **provided the render thread holds no lock the
  restore needs**: keep it writing straight to `stdout`, never through a mutex the hook would
  take. Release sets `panic = "abort"`, so no `Drop` can stand in.

### 3.4 Keep the escape hatch working, and extend it to the renderer

[`app/input_reader.rs:27-204`](../../../crates/cyrup-tui/src/app/input_reader.rs) is a
`std::thread` that hard-exits on three unserviced `Ctrl+C`/`Ctrl+D` chords and prints
``cyrup: run loop wedged in arm `{arm}` for {elapsed}`` from `ACTIVE_ARM` (`:110-111`). It must
survive: `ArmGuard::enter("events")`
([`run_action.rs:293`](../../../crates/cyrup-tui/src/app/run_action.rs)) still brackets the
fold, and the hatch must now also restore the terminal correctly when the *render* thread is
stuck.

Mirror the existing machinery rather than inventing a second one — same
`Mutex<Option<…>>` shape, same `try_lock`-never-`lock` discipline:

```rust
/// The frame currently being painted, and since when — written by the render thread, read by
/// the input reader's watchdog. Same shape and same discipline as `ACTIVE_ARM`
/// (`input_reader.rs:110-111`), so a wedged renderer names itself instead of presenting as a
/// silent freeze. `hard_exit_from_reader` (`:194-205`) reports both.
pub(crate) static ACTIVE_FRAME: std::sync::Mutex<Option<std::time::Instant>> =
    std::sync::Mutex::new(None);
```

`mark_input_serviced` (`:52`) keeps meaning "serviced" and stays **after** the frame request —
a *requested* frame the user never sees is not service, exactly as TUI-092 §7 property 7
argues about a *drawn* one.

### 3.5 Correct the comments that this change makes false

Four in-code comments become actively misleading and must be rewritten **in the same change**,
or the next reader re-derives the wrong model:

* [`run.rs:293-301`](../../../crates/cyrup-tui/src/app/run.rs) — the `biased;` rationale
  currently tells the reader that arm ORDER is what protects the keyboard. Verified verbatim
  at HEAD: *"as soon as one `draw_synchronized` costs more than a tick — which is what growing
  transcripts do — the input arm is never reached again and the keyboard dies while the screen
  keeps animating. Do NOT 'tidy' the input arm back down among the tickers."* After §3.1 that
  is no longer the mechanism; the frame cap and the preempting input request are. The ordering
  remains load-bearing for the cancel arm and the session-swap arm and that half must stay —
  `run_loop_input_priority.rs` still pins it.
* **The SECOND copy of that rationale**, on the input arm itself
  ([`run.rs:304-315`](../../../crates/cyrup-tui/src/app/run.rs)), which ends *"this arm ends in
  `draw_synchronized()` anyway — so servicing a key repaints the frame the spinner would have
  drawn"*. After §3.1 the arm ends in `request_immediate()`, so the sentence is false as
  written even though its conclusion still holds. AUG-2 missed this one.
* [`transcript/cache.rs:36-40`](../../../crates/cyrup-tui/src/transcript/cache.rs) —
  *"`markdown::render` emits ONE un-wrapped `Line` per prose paragraph"*. Already false at HEAD
  (§0.2 — `walk.rs:837` wraps); after §3.0 it describes deleted code.
* [`app/draw.rs:277-281`](../../../crates/cyrup-tui/src/app/draw.rs) — the same false claim on
  the commit path (*"`entry_lines` emits one un-wrapped `Line` per prose paragraph"*), used
  there to justify the `.wrap()` §3.0 removes.

---

## 4. Sizing

| stage | risk | benefit | evidence |
| --- | --- | --- | --- |
| **§3.0a** bound the highlight to `shown` | **lowest** — one parameter, two call sites, output proven identical | **187×** on a collapsed 2,000-line `read`: 356.88 ms → 1.9 ms/frame, 99.5% of the work was discarded | measured + equivalence-asserted, §0.7 B2/B3 |
| **§3.0b** resumable + memoised syntect | **low** — one file, contained behind `highlight_inner`, fallbacks untouched | **66–3078×** on the streaming fence; **99–233×** on an expanded tool body; removes the term that crosses the spinner tick | measured, §0.4 / §0.7 B1 / §3.0b B4 |
| **§3.0** wrapped-row cache + windowed paint | **low-medium** — one cache shape, no terminal, no threads, but **18 test assertions re-anchor** (§0.8) | **~180 ms → ~194 µs** at a 16k-row turn; frame becomes constant in turn size; closes the §0.5 scroll bug and makes PROSE-WRAP clipping unrepresentable | measured, §0.2 / §0.3 / §0.8 |
| **§3.1** frame scheduler | **low** — one struct, one loop statement, 26 call-site edits, 4 structural guards re-anchored | frames capped at 62.5 Hz; coalescing across arms, not just within one | pi parity, `tui.ts:343,772-822,896-900` |
| **§3.2** `FrameState` publish | **medium** — every chrome component gains a `lines()` seam | none alone; a prerequisite | — |
| **§3.3** render thread | **high** — terminal ownership, shutdown ordering, panic safety, escape hatch | only against a **blocking tty write**, which nothing above touches | argued, §1; not measured |

**Do §3.0a, §3.0b, §3.0 and §3.1 unconditionally, in that order.** §3.0a first because it is
the smallest edit in the task and it beats every other stage on ratio, with an equivalence proof
rather than an argument. §3.0b second because §0.4/§0.7 show the syntect term is what actually
crosses the spinner tick, and it lands in one file. §3.0 third because it is what makes §3.2's
publish an `Arc::clone`. Then re-measure before committing to §3.2/§3.3.

**§3.0a and §3.0b compose but do not overlap**: §3.0a shrinks the *input* to the highlighter,
§3.0b removes the *repetition*. A collapsed 2,000-line `read` inside a streaming turn pays
356.88 ms today, 1.9 ms after §3.0a alone, and a memo-hit clone (~15 µs at 10 rows) after both.

The earlier recommendation to land [PERF-001](PERF-001_STREAM_SNAPSHOT_QUADRATIC.md) first
still holds for §3.2/§3.3 and does not hold for §3.0b/§3.0: PERF-001 removes provider-side CPU
from the same pipeline, but nothing in it touches the re-highlight or the redundant wraps.

---

## 5. Definition of Done

### 5.1 After §3.0a — the highlighter's input

0. **Nothing is highlighted that is not rendered.** `highlight_code_lines` takes a `max_rows`
   bound; both `tool_builtin.rs` call sites compute `shown` **before** the highlight and pass
   it. `grep -n 'highlight_code_lines' crates/cyrup-tui/src/transcript/tool_builtin.rs` shows
   `shown` as the last argument at both sites.
0b. **The rendered output is byte-identical.** The existing `tool_builtin` / `read` / `edit`
   preview tests pass unchanged — they are the equivalence check, and §0.7 B3 is the reason to
   expect them to. If any of them changes output, `max_rows` was threaded below `shown`.

### 5.1b After §3.0b — the markdown miss path

1. **A streaming frame re-parses only new code lines.** `ParseState::new(` appears in
   `markdown/highlight.rs` only on the cursor's rebuild path, never once per call:
   `grep -c 'ParseState::new' crates/cyrup-tui/src/markdown/highlight.rs` is 1.
2. **The invalidation key is complete.** The cursor rebuilds on a language change, a
   `theme.generation` change, and on any input that is not a strict line-prefix extension of
   what it already consumed. The memo key additionally carries `len` alongside `hash`, so a
   hash collision cannot serve wrong rows.
3. **The fallbacks are untouched.** An empty or unknown language token still takes
   `highlight_lines`' `flat()` branch; `highlight_code_lines` still returns `None` for the same
   and still strips the 2-space gutter (T5 / TUI-FIDELITY §2 span semantics unchanged).
4. **An open streaming fence stays correct.** The final, still-growing line is re-parsed each
   frame rather than consumed into the cursor.
4b. **The memo is bounded and cannot panic.** `MEMO_CAP` entries, evicted by insertion order;
   the `RefCell` is entered with `try_borrow_mut` and the failure path is today's uncached
   highlight, so re-entrancy degrades performance and never correctness. `cargo clippy` exit 0
   is the check that no `borrow_mut`/`unwrap` slipped in — `clippy::panic` does **not** fire
   under `cargo test`.
4c. **Both consumers benefit.** A settled tool body re-renders from the memo, not from syntect:
   after a `read` completes, subsequent streaming frames in the same turn do not re-parse it.

### 5.2 After §3.0 — frame cost

5. **A frame is constant in active-turn size.** The `Paragraph` in `TranscriptView::render`
   receives at most `area.height` rows, carries no `.wrap()` and no `.scroll()`, and the rows
   reach it through `Arc::clone`.
   `grep -n 'lines.clone()\|\.wrap(Wrap' crates/cyrup-tui/src/transcript/cache.rs` returns
   nothing.
6. **Content is wrapped exactly once per materialisation**, through `wrap_all_owned`, which
   **moves** every already-fitting row. The `wrapped_height` measurement pass is gone from both
   `cached_render` (`cache.rs:42`) and `flush_committed` (`app/draw.rs:282`); the free function
   survives for its five other direct callers (§3.0 hazard 4).
7. **Height has one definition.** `rows.len()` is both the scroll bound and `content_height`;
   no `lines.len()` remains on either path. A long single-paragraph streaming answer reserves
   its full wrapped height in the inline region, is tail-anchored on the newest text (§0.5),
   and is not clipped on commit (R-ARCH-TUI-003/-005).
7b. **The height tests are re-anchored, not deleted.** `transcript/tests/render_cache.rs`
   compares against `wrap_all_owned(...).len()` — the new oracle — in all 18 places (§0.8), and
   `tests/assembled_render.rs:250,281`, `transcript/tests/progressive_commit.rs:35,41` and
   `transcript/tests/osc_hyperlinks.rs:279` pass **unmodified**. Those three are the
   independent check; if any needs editing to pass, the height changed and §3.0 regressed.
8. **OSC-8 links still land on the right text** — the href table is cached with the rows it was
   built from, and injection still runs after the blit.

### 5.3 After §3.1 — frame rate

9. **N state changes inside one 16 ms window produce one frame.** The only unconditional
   production paint is the top-of-body site in `App::run`; every arm requests.
10. **A keystroke is never delayed by the throttle.** The input arm forces an immediate frame
    (pi `tui.ts:896-900`), and `mark_input_serviced` still fires after it.
11. **The five synchronous sites remain synchronous** — `run.rs:120`, `run.rs:419`,
    `crossterm.rs:136`/`:150`/`:170` (pi's `renderNow`) — and the exit path flushes a pending
    frame before `drain_and_restore`.
12. **The four structural guards are re-anchored and passing**, not deleted (§3.1 table).

### 5.4 After §3.2/§3.3 — the decoupling

13. **Input cannot be starved by draw cost** — structurally, not "does not reproduce".
14. **The agent is not throttled by the renderer**: `Fanout::emit`'s awaited sends do not stall
    the provider stream behind a slow frame.
15. **Terminal state is correct on every exit path**: normal quit, `Ctrl+D`, double-tap
    `Ctrl+C`, the three-chord hard exit, a panic on the run-loop task, and a panic on the
    **render thread**.
16. **The wedge detector still reports**, and a stuck *render thread* is named rather than
    appearing as a silent freeze.
17. **No tokio worker does terminal I/O**; the render thread is an OS thread; it paints nothing
    while `TERMINAL_RELEASED` is set; alternate-screen transitions, resizes and `append_lines`
    anchoring are ordered against frames.

### 5.5 The gate, and the go/no-go for §3.3

18. **The suite is green under the real gate**, from the workspace root:

    ```bash
    cargo test --workspace --features test-fixtures --no-fail-fast
    cargo clippy --workspace --all-targets --features test-fixtures; echo "exit=$?"   # MUST be 0
    ```

    `--features test-fixtures` is required or two `[[bin]]` targets never build. The no-panic
    lints (`unwrap_used`, `expect_used`, `panic`, `indexing_slicing`) fire **only** under
    clippy — check the exit code, not the output.
19. **The go/no-go.** With §3.0a, §3.0b and §3.0 landed, a turn holding 2,000 lines of code
    should cost ~194 µs of layout and ~130 µs of highlight per frame instead of ~350 ms, and a
    collapsed 2,000-line `read` in the same turn should cost ~1.9 ms instead of ~357 ms. If the
    spinner no longer slips at the largest reachable transcript, §3.3 is **not** justified by
    CPU and the remaining case rests on tty blocking alone (§1) — record that and decide
    deliberately rather than by momentum.

---

## 6. The probes

All five are standalone crates under `./tmp`, no workspace edit, `--release`. Each reproduces
the production function **verbatim** rather than approximating it.

| probe | reproduces | key result |
| --- | --- | --- |
| [`tmp/perf005-probe`](../../../tmp/perf005-probe) | §0.2 — the hit path, A→D | 91 ms → 194 µs at 16k rows |
| [`tmp/perf005-miss`](../../../tmp/perf005-miss) | §0.3 — `wrapped_height` (E), naive vs move-based wrap (F/G) | E = 88 ms at 16k; G is 1.7× F |
| [`tmp/perf005-hl`](../../../tmp/perf005-hl) | §0.4 — `highlight_inner` verbatim, and the resumable form | 174 µs / code line; 87 ms at 500 lines |
| **[`tmp/perf005-hl2`](../../../tmp/perf005-hl2)** *(AUG-3)* | §0.7 — memo hit vs re-highlight (B1), `shown`-bounded vs full (B2), **equivalence assertion** (B3), resumable cursor (B4) | 99–233× memo; **99.5% of a collapsed body's highlight is discarded**; 66–3078× resumable |
| **[`tmp/perf005-wrapeq`](../../../tmp/perf005-wrapeq)** *(AUG-3)* | §0.8 — `wrap_line` + `wrapped_height` verbatim, 22 shapes × width sweep | **20/22 agree**; the 2 that do not are a tab (unreachable) and a whitespace-only over-width row |

`perf005-miss`, `perf005-hl`, `perf005-hl2` and `perf005-wrapeq` need
`ratatui = { version = "0.30.2", features = ["unstable-rendered-line-info"] }` — `line_count`
is behind that feature, which `cyrup-tui/Cargo.toml:84` already enables.

**Four traps.**

* **Measure in `--release`.** A debug allocator's noise swamps the C→D difference entirely.
* **Measure the frame, not the clone.** The deep clone is ~4% of the hit path; a probe that
  times `lines.clone()` alone reports the least important of the three costs as the finding.
  That is how the first round nearly stopped at the wrong fix.
* **Measure the MISS, not the hit.** `push_assistant_delta` bumps the generation, so the hit
  path is not the streaming path. A probe that only exercises `RenderCache` hits misses the
  syntect term entirely — which is the whole of §0.4, and 4× everything else combined.
* **[AUG-3] Enumerate the CALL SITES, not the function.** §0.4 priced `highlight_inner`
  correctly and still missed 99.5% of the available win, because it followed one of the two
  paths that reach it. Before pricing a hot function, `grep` for every caller and ask whether
  each one's content is static or growing — they need different fixes, and the static one was
  both cheaper and bigger here.

---

/home/d0m17bw/workspace/cyrup/.flux/todo/performance/PERF-005_DECOUPLE_RENDER_FROM_FOLD.md
