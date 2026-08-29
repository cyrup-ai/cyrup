---
stage: aug
status: in-progress
updated: 2026-08-29 05:10
aug_against: cyrup HEAD 8f49433 · pi v0.84.1 (`packages/tui/src/tui.ts`) · numbers measured on this host, `--release`
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
> **[AUG] That sentence is now measured, not projected.** One inline frame costs
> **11.5 ms at a 2,000-row active turn, 46.6 ms at 8,000 and 92.1 ms at 16,000** — the 80 ms
> tick is crossed at ~14,000 rows and the 16.7 ms 60 Hz budget at ~2,900. The cost is
> **linear in active-turn size on every frame, cache hit included**, and §0.2 shows it is
> almost entirely *redundant work*. Fixing that (§3.0) makes a frame **flat at ~190 µs** —
> constant in turn size, a **122× reduction** at 16k rows — and it touches no terminal
> ownership at all.

---

## 0. READ THIS FIRST — three things, and the last two change the plan

### 0.1 Do not re-do TUI-092

**[`docs/gap-analysis/bugs/TUI-092-progressive-lockup.md`](../../../docs/gap-analysis/bugs/TUI-092-progressive-lockup.md)
is required reading before touching this.** All eight of its fixes have landed. Every one of
the obvious "make the TUI faster" moves is already made, and re-doing any of them is wasted
work:

| | already landed | do not redo |
| --- | --- | --- |
| F1 | scrollback accumulator gated out of production builds | — |
| F2 | `RenderCache` keyed `(generation, width, theme.generation)` — no triple materialisation per frame | the render cache exists |
| F3 | drain-then-draw on the `events`/`input`/`bash_next` arms — N deltas cost N folds and **one** frame | the coalescing exists |
| F4 | `context_usage` reverse scan, zero message clones | — |
| F5 | ratatui `scrolling-regions` on by default | — |
| F6 | `BashExecution::output_lines` bounded at 2000 | — |
| F7 | image protocol memoised per frame | — |
| F8 | by-value event ingest, payloads moved not cloned | — |

**What TUI-092 did was reduce frame cost and frame count. What it did not do is remove the
coupling.** Draw still happens inline on the task that folds events
([`run_action.rs:339`](../../../crates/cyrup-tui/src/app/run_action.rs)), so the starvation
mode is mitigated, not eliminated.

Also from that doc's §8: **the original defect has not been re-observed since round 2
landed, and nobody has confirmed the fix live** — the workspace's standing rule is that a
TUI claim is not settled by `TestBackend`.

### 0.2 [AUG] F2's own definition of done is FALSE — a frame is O(active turn), even on a cache hit

TUI-092 §7 property 2 claims *"a frame with unchanged state is O(changed chrome)"* and
offers `cached_render`'s key check as *"the whole proof"*. It is not. The cache memoises the
**markdown + syntect materialisation**; it memoises nothing downstream of it, and
[`TranscriptView::render`](../../../crates/cyrup-tui/src/transcript/cache.rs) pays three
costs on **every** frame, hit or miss:

```rust
// crates/cyrup-tui/src/transcript/cache.rs:228-244
fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
    let width = area.width as usize;
    let (total, lines) = {
        let cache = self.cached_render(width, theme);
        (cache.lines.len(), cache.lines.clone())      // (1) DEEP clone of every line + span
    };
    …
    let para = Paragraph::new(lines)
        .style(theme.base_style())
        .wrap(Wrap { trim: false })                   // (2) re-wraps the WHOLE turn …
        .scroll((scroll, 0));                         // (3) … to paint ≤ area.height rows
    frame.render_widget(para, area);
```

1. **`cache.lines.clone()` is a deep copy.** `Line<'static>` owns `Vec<Span<'static>>` and
   every `Span` holds a `Cow::Owned(String)` — markdown and syntect both emit owned
   strings — so this allocates once per span and memcpys every byte, per frame.
2. **`.wrap(Wrap { trim: false })` re-wraps content that is already wrapped.**
   [`MdRenderer::finish`](../../../crates/cyrup-tui/src/markdown/walk.rs) ends the token walk
   with a full second pass — `self.out.into_iter().flat_map(|l| wrap_line(&l, width))` at
   `walk.rs:834-838` — and the module doc states the consequence outright
   ([`markdown/mod.rs:98-101`](../../../crates/cyrup-tui/src/markdown/mod.rs)):
   *"Rows come back already wrapped to `width` … nothing downstream needs to reflow them, and
   reflowing them at a wider width is exactly the L2/M10 bug."* The comment in
   `cached_render` that justifies the second wrap — *"`markdown::render` emits ONE un-wrapped
   `Line` per prose paragraph"* — is **stale**; it describes a renderer that no longer exists.
3. **Nothing is windowed.** The entire `Vec<Line>` is handed to `Paragraph`, which wraps all
   of it and then discards everything above `scroll`.

Measured on this host, `--release`, painting a 100 × 30 area (§6 reproduces it):

| active-turn rows | A: today | B: drop the deep clone | C: + drop the re-wrap | D: + window the paint |
| --- | --- | --- | --- | --- |
| 20 | **183 µs** | 177 µs | 145 µs | **141 µs** |
| 200 | **1 167 µs** | 1 160 µs | 197 µs | **189 µs** |
| 1 000 | **5 704 µs** | 5 636 µs | 337 µs | **189 µs** |
| 2 000 | **11 472 µs** | 11 293 µs | 545 µs | **194 µs** |
| 4 000 | **22 862 µs** | 22 344 µs | 991 µs | **193 µs** |
| 8 000 | **46 644 µs** | 44 503 µs | 1 895 µs | **190 µs** |
| 16 000 | **92 139 µs** | 88 409 µs | 4 535 µs | **188 µs** |

Read the table in this order, because the ranking is counter-intuitive:

* **The deep clone is NOT the problem** (A→B is ~4%), even though in isolation it costs
  2.5 ms at 4k rows and 12.1 ms at 16k. It is dwarfed by the thing after it.
* **The redundant re-wrap is 95% of the frame** (B→C is 20× at 16k rows). ratatui's
  `WordWrapper` walks every line before honouring `.scroll()`, so the app re-flows the whole
  turn to paint thirty rows — work whose *only* output is the rows it then throws away.
* **Windowing removes the last O(n)** (C→D): the frame becomes **flat at ~190 µs**,
  independent of turn size. That ~190 µs is `Buffer::reset()` over 3,000 cells plus the blit;
  it is the floor, not the content.

**This is the crux of the whole task.** The in-code warning at `run.rs:293-301` is real
*because* frame cost grows with the turn. Remove the growth and the cliff is no longer
reachable by a transcript getting longer — which is the only mechanism anyone has proposed
for it. §3.0 is therefore not a preliminary; it is the fix with the evidence behind it, and
the thread in §3.3 is what is left over afterwards.

**The same three costs exist on the commit path** and should be fixed in the same change:
[`flush_committed`](../../../crates/cyrup-tui/src/app/draw.rs) at `draw.rs:248-292` builds
`lines`, calls `wrapped_height(&lines, width)` — which does `lines.to_vec()`, a second deep
clone, then a full `Paragraph::line_count` wrap
([`transcript/layout.rs:384-391`](../../../crates/cyrup-tui/src/transcript/layout.rs)) — and
then wraps a *third* time inside `insert_before`'s `Paragraph::new(lines).wrap(…)`.

### 0.3 [AUG] §2's premise is half wrong: pi DOES decouple render from fold

pi has a **frame scheduler with a hard frame-rate cap**, and cyrup ported none of it. This is
unported behaviour, not a language difference:

```ts
// pi/packages/tui/src/tui.ts:343
private static readonly MIN_RENDER_INTERVAL_MS = 16;

// :772-781 — the default path. Sets a flag; paints nothing.
requestRender(force = false): void {
    if (force) { this.resetRenderState(); this.requestImmediateRender(); return; }
    if (this.renderRequested) return;
    this.renderRequested = true;
    process.nextTick(() => this.scheduleRender());
}

// :806-822 — one frame per MIN_RENDER_INTERVAL_MS, no matter how many requests arrived
private scheduleRender(): void {
    if (this.stopped || this.renderTimer || !this.renderRequested) return;
    const elapsed = performance.now() - this.lastRenderAt;
    const delay = Math.max(0, TuiBase.MIN_RENDER_INTERVAL_MS - elapsed);
    this.renderTimer = setTimeout(() => { … this.doRender(); if (this.renderRequested) this.scheduleRender(); }, delay);
}

// :896-900 — and input PREEMPTS the throttle, with the reason stated
this.focusedComponent.handleInput(data);
// Keyboard input is latency-sensitive. Avoid the throttled timer path,
// where even setTimeout(0) can take a full 16 ms tick on Windows.
this.requestImmediateRender();
```

`interactive-mode.ts` calls `requestRender(` **106 times** and none of them paints
synchronously; the one synchronous escape hatch is `renderNow()`, used once
([`interactive-mode.ts:815`](../../../../pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts)).

cyrup has **30 production `draw_synchronized()` call sites**
(`run_arms.rs` 22, `crossterm.rs` 3, `run.rs` 3, `run_action.rs` 2), **every one of which
paints immediately**, and no frame cap anywhere in the crate
(`grep -rn 'MIN_RENDER\|frame_interval\|render_interval' crates/cyrup-tui/src` → nothing).

```bash
# the census, reproducible:
grep -rn 'draw_synchronized()' crates/cyrup-tui/src --include=*.rs \
  | grep -v '^crates/cyrup-tui/src/tests' | grep -v 'fn draw_synchronized' \
  | grep -v '// ' | sed 's/:.*//' | sort | uniq -c
```

So: pi cannot get *parallelism* — that part of §2 stands — but it does get **coalescing
across arms and a bounded frame rate**, which cyrup does not. That is a straight port, it is
cheap, and it subsumes F3's within-arm drain with a structural guarantee.

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

`draw_synchronized` is [`app/crossterm.rs:87`](../../../crates/cyrup-tui/src/app/crossterm.rs).
Every call site is on the run-loop task.

The consequence chain:

1. Draw cost grows with active-turn size — **§0.2, measured: 11.5 ms at 2k rows, 92 ms at
   16k, on every frame including a `RenderCache` hit.**
2. The run loop is `biased;` ([`run.rs:302`](../../../crates/cyrup-tui/src/app/run.rs)), and
   the spinner ticker re-arms every 80 ms
   ([`status_indicator.rs:48`](../../../crates/cyrup-tui/src/status_indicator.rs)).
3. If one draw exceeds one tick, arms below the ticker are reached less often; the input arm
   is deliberately hoisted above the tickers to prevent exactly this, which is a mitigation
   of the coupling rather than a removal of it.
4. Meanwhile `Fanout::emit` **awaits** every send
   ([`subscriber.rs:63-72`](../../../crates/cyrup-session-svc/src/subscriber.rs), *"backpressure
   → slows the agent, never drops"*), so a slow draw does not merely look bad — **it throttles
   the provider stream**.

That last point is what makes this a throughput task and not a cosmetics task.

**[AUG] And there is one cost the CPU numbers do not capture at all.**
`CrosstermBackend` writes straight to `io::Stdout`, and a `write(2)` to a tty **blocks
without bound** when the terminal is not draining — flow control (`Ctrl+S`/XOFF), a slow ssh
link, a suspended terminal emulator. That stall is not proportional to anything and no amount
of §3.0 removes it. **It is the honest justification for §3.3's OS thread**, and it should be
the argument that decides whether §3.3 gets built — not the CPU cliff, which §3.0 closes.

---

## 2. What pi can and cannot do — CORRECTED

pi has one event-loop thread. `Promise.all` is concurrency, not parallelism; its renderer,
its fold and its tool execution all contend for that thread by construction.

**But it does not paint inline.** §0.3: `requestRender` defers, `MIN_RENDER_INTERVAL_MS`
caps the rate, `requestImmediateRender` gives input a preempting path. Every claim of the
form *"pi cannot do this"* applies only to §3.3 — putting the terminal writes on a real
thread so a blocking `write(2)` cannot reach the fold. §3.1 is not a cyrup innovation; it is
a port of behaviour cyrup skipped.

---

## 3. Required implementation

Four stages, in this order. Each stands alone and each is a strict prerequisite of the next:
§3.0 makes a frame cheap enough that a cap is meaningful, §3.1 makes frames rare and gives
input a preempting path, §3.2 makes a frame publishable, §3.3 moves the writes off the task.

### 3.0 Make a frame O(visible), not O(active turn)

**Wrap once, into the cache. Share the rows. Paint only what fits.**

[`transcript/mod.rs:323-333`](../../../crates/cyrup-tui/src/transcript/mod.rs) — the cache
holds **already-wrapped display rows**, behind an `Arc` so no consumer ever deep-copies it:

```rust
struct RenderCache {
    generation: u64,
    width: usize,
    theme_generation: u64,
    /// ALREADY-WRAPPED display rows, one `Line` per screen row, shared not copied.
    /// `Arc` because every consumer wants the whole vector and none of them mutates it:
    /// `TranscriptView::render` (per frame), `content_height` (per frame) and — after §3.2 —
    /// the published `FrameState`. `Arc::clone` is 11 ns against 12.1 ms for the deep copy
    /// this replaces at a 16k-row turn.
    rows: Arc<Vec<Line<'static>>>,
    links: crate::osc::LinkSink,
}
```

`wrapped_height` disappears as a separate concept: it is `rows.len()`.

[`transcript/cache.rs:28-56`](../../../crates/cyrup-tui/src/transcript/cache.rs) —
`cached_render` does the wrap on the miss path, where the markdown pass already is:

```rust
        if stale {
            let links = crate::osc::LinkSink::new();
            let lines = self.lines_with(width, theme, Some(&links));
            // Wrap ONCE, here, into the cache. `wrap_line` early-returns a verbatim clone for a
            // row that already fits (`transcript/layout.rs:53-55`), so this is a no-op for the
            // markdown bodies — `MdRenderer::finish` already wrapped them to the content width
            // (`markdown/walk.rs:834-838`) — and only bites the rows the inner wrap could not
            // bound. It replaces BOTH the per-frame `Paragraph::wrap` and the `wrapped_height`
            // measurement pass, which were two full wraps of the same content.
            let rows: Vec<Line<'static>> = lines
                .into_iter()
                .flat_map(|line| crate::transcript::wrap_line(&line, width.max(1)))
                .collect();
            self.render_cache = RenderCache {
                generation: self.render_generation,
                width,
                theme_generation: theme.generation,
                rows: Arc::new(rows),
                links,
            };
        }
```

[`transcript/cache.rs:228-249`](../../../crates/cyrup-tui/src/transcript/cache.rs) — the
render becomes a refcount bump, a slice and a bounded blit:

```rust
    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
        let width = area.width as usize;
        let inner_h = area.height as usize;
        // 11 ns. The rows outlive the borrow, so nothing here holds `&mut self` across the paint.
        let rows = Arc::clone(&self.cached_render(width, theme).rows);
        let total = rows.len();
        let max_scroll = total.saturating_sub(inner_h);
        self.scroll_offset = self.scroll_offset.min(max_scroll);
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

`.get(..)` rather than `[..]` — `cyrup-tui` denies `clippy::indexing_slicing` and
`clippy::string_slice` ([`lib.rs:46-50`](../../../crates/cyrup-tui/src/lib.rs)).

**Do the same on the commit path** ([`app/draw.rs:248-292`](../../../crates/cyrup-tui/src/app/draw.rs)):
build `rows` with the same `flat_map(wrap_line)`, use `rows.len()` as the `insert_before`
height, and drop `.wrap(Wrap { trim: false })` from the `Paragraph`. That removes one deep
clone (`wrapped_height`'s `lines.to_vec()`) and two of the three wraps per commit.

Three hazards, each of which will look like a rendering regression if missed:

* **`content_height` must keep meaning wrapped rows.** It feeds `live_region_height` →
  `region_constraints` → the inline viewport height
  ([`app/layout.rs:174`](../../../crates/cyrup-tui/src/app/layout.rs),
  [`app/draw.rs:56-140`](../../../crates/cyrup-tui/src/app/draw.rs)). `rows.len()` is the same
  quantity `wrapped_height` computed, now exactly rather than by re-measurement — but a
  `lines.len()` left anywhere would silently under-size the viewport and reintroduce the
  PROSE-WRAP truncation the `wrapped_height` comment was written for.
* **`osc::inject` alignment.** Its doc requires that the marked cells exist before injection
  and that `Buffer::diff_iter` stays column-aligned. Slicing changes *which* rows reach the
  buffer, not their cell layout, so the contract holds — but the injection must still run
  after `render_widget`, exactly as it does now.
* **Non-markdown rows.** Tool bodies and the live bash block reach `lines_with` through
  `tool_lines`/`render_lines`, which are *not* guaranteed pre-wrapped. That is precisely why
  the wrap moves into the cache instead of being deleted: it stays correct for them and
  becomes free for everything else.

### 3.1 Port pi's frame scheduler — request, don't paint

One frame per `MIN_RENDER_INTERVAL`, input preempts, and a `renderNow` escape hatch for the
paths that must have pixels before they return.

```rust
/// pi `TuiBase.MIN_RENDER_INTERVAL_MS` (`packages/tui/src/tui.ts:343`) — a 62.5 Hz cap.
pub(crate) const MIN_RENDER_INTERVAL: Duration = Duration::from_millis(16);

/// pi's `renderRequested` / `renderTimer` / `lastRenderAt` triple (`tui.ts:772-822`), owned by
/// the run loop. A request is a FLAG, never a paint: N arms firing inside one interval produce
/// one frame, which is F3's guarantee upheld across arms rather than inside each one.
pub(crate) struct FrameScheduler {
    requested: bool,
    /// pi's `requestImmediateRender` (`tui.ts:783-796`): a keystroke must not wait out the
    /// throttle. Set only by the input arm.
    force: bool,
    last: Instant,
}

impl FrameScheduler {
    pub(crate) fn request(&mut self) { self.requested = true; }
    pub(crate) fn request_immediate(&mut self) { self.requested = true; self.force = true; }
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
    pub(crate) fn taken(&mut self) { self.requested = false; self.force = false; self.last = Instant::now(); }
}
```

Wiring, in [`app/run.rs`](../../../crates/cyrup-tui/src/app/run.rs):

```rust
        'run: loop {
            self.drain_over_budget_arm();
            // The ONE production frame site. At the top of the body, so it batches every arm
            // that fired on the previous iteration regardless of which one it was — pi's
            // `scheduleRender` callback, in the one place cyrup has to put it.
            if self.frames.due() {
                self.draw_synchronized()?;
                self.frames.taken();
            }
            // Wakes the loop for a frame that is pending but not yet due. Never resolves when
            // nothing is requested, so an idle session costs no wakeups — the same shape as
            // `overlay_ticked` / `alt_timer` above it.
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
frames starvable by any hotter arm above it under `biased;`, which is the exact failure this
task exists to remove.

Then convert the call sites:

* **25 sites become `self.frames.request();`** — pi's `requestRender()`: all 22 in
  `run_arms.rs`, the events arm at
  [`run_action.rs:339`](../../../crates/cyrup-tui/src/app/run_action.rs), and
  `on_altscreen_tick` at [`run.rs:453`](../../../crates/cyrup-tui/src/app/run.rs) (an ordinary
  arm despite living in `run.rs`).
* **The input arm** ([`run_action.rs:268`](../../../crates/cyrup-tui/src/app/run_action.rs))
  becomes `self.frames.request_immediate();` — pi's `requestImmediateRender()` at
  `tui.ts:896-900`, for the reason pi states in that comment.
* **Five sites stay synchronous `draw_synchronized()`** — pi's `renderNow()`
  (`interactive-mode.ts:815`) — because control leaves the loop immediately afterwards and a
  deferred frame would never land: the seed frame at
  [`run.rs:120`](../../../crates/cyrup-tui/src/app/run.rs); the post-`stop_fullscreen` frame on
  the exit path at [`run.rs:419`](../../../crates/cyrup-tui/src/app/run.rs); and the three
  terminal-handed-back redraws in
  [`crossterm.rs:136`](../../../crates/cyrup-tui/src/app/crossterm.rs) (`suspend`, after `fg`),
  [`:150`](../../../crates/cyrup-tui/src/app/crossterm.rs) (`open_external_editor`) and
  [`:170`](../../../crates/cyrup-tui/src/app/crossterm.rs)
  (`open_external_editor_for_selector`).
* **The exit path must flush a pending frame** before `drain_and_restore`, or the last state
  change before a quit is never drawn.

`ArmGuard::enter` ([`app/input_reader.rs:111-142`](../../../crates/cyrup-tui/src/app/input_reader.rs))
keeps bracketing the arm bodies; add a `ArmGuard::enter("frame")` around the top-of-body draw
so the wedge detector can still name it.

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
    /// Materialised chrome, all of it viewport-bounded: band, editor, selector/loader slot,
    /// completion popup, extension header/footer/widgets, image strip, overlays.
    chrome: Chrome,
    /// `[header, msg, pending, band, images, wabove, slot, popup, wbelow, footer]` — the exact
    /// output of `region_constraints`, resolved once by the fold so the two sides cannot
    /// disagree on row counts (`app/layout.rs`'s idempotence note).
    regions: [u16; 10],
    geometry: Geometry,      // term_w, term_h, viewport_height, live_floor
    cursor: Option<Position>,
    theme: Arc<UiTheme>,
    /// The `insert_before` payload, pre-wrapped by §3.0, or empty on a non-commit frame.
    commits: Arc<Vec<Line<'static>>>,
}
```

**The blocker to plan around, stated plainly: cyrup's render path is `&mut`.**
[`render(frame, state: &mut AppState)`](../../../crates/cyrup-tui/src/app/render.rs) at
`render.rs:4`, and [`Component::render`](../../../crates/cyrup-tui/src/component.rs) at
`component.rs:19` is `fn render(&mut self, …)`. The transcript mutates `scroll_offset` and its
cache, the editor mutates its wrap state, the selector and every overlay mutate as they paint.
So the render thread **cannot** be handed an `&AppState`, and `FrameState` cannot be a view —
it has to be the *materialised output* of those components.

The required shape: give each chrome component a
`fn lines(&mut self, width: u16, theme: &UiTheme) -> Vec<Line<'static>>` that its existing
`render` then blits, and have the fold call `lines()` while the render thread blits. This is
affordable **only** because every one of those components is viewport-bounded — the editor
caps itself at `max(5, rows * 3 / 10)` ([`app/layout.rs:25`](../../../crates/cyrup-tui/src/app/layout.rs)),
the band is 2 rows, the footer 1 — whereas the transcript is not, which is why the transcript
rides the `Arc` from §3.0 instead.

Two carries that are easy to lose:

* **The selector caret.** [`app/render.rs`](../../../crates/cyrup-tui/src/app/render.rs) derives
  it by scanning the *rendered buffer* (`crate::selector::caret_cell(frame.buffer_mut(), slot_area)`).
  That scan must run on the render thread after its blit, or the caret must be produced
  directly by the selector's `lines()`. Do the latter; a buffer scan on the render thread
  re-introduces a render-side computation that the publish was supposed to have settled.
* **`publish_extension_readbacks`** runs at the top of `App::draw` (`draw.rs:56-66`) so a guest
  reading the editor buffer or theme name sees what the frame is about to show. It is fold
  state, not paint: it moves to the publish, not to the thread.

### 3.3 Run the terminal writes on a dedicated OS thread

A `std::thread` (not a tokio task — it does blocking terminal I/O and must not occupy a
runtime worker) that wakes on a publish notification or a ~60 Hz timer, `load()`s the current
`Arc<FrameState>`, and draws. Terminal ownership moves entirely to this thread; nothing else
may write to the terminal concurrently.

**Build this only if §5's exit criterion says so** (see §4). The justification is not the CPU
cliff — §3.0 closes that — it is the unbounded blocking `write(2)` named at the end of §1.

`self.terminal` has **21 uses across 8 files**, which is the tractable half. The untracked
half is everything that writes escapes straight to `io::stdout()` and must now be ordered
against frames:

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

- **Alternate-screen enter/leave, raw-mode toggles, and terminal restore** must be ordered
  against frames. Today they are trivially ordered because everything is on one task. See
  [`app/crossterm.rs:110-112,200`](../../../crates/cyrup-tui/src/app/crossterm.rs) (the
  by-design block announced to the wedge detector) and
  [`app/backend.rs`](../../../crates/cyrup-tui/src/app/backend.rs)'s `append_lines`
  anchoring, which depends on knowing exactly what was drawn last. Model them as **commands on
  the render thread's own queue**, not as calls that race it; the anchor
  ([`InlineBackend::anchor`](../../../crates/cyrup-tui/src/app/backend.rs), TUI-093) is
  render-thread-private state and a second writer invalidates it.
- **`TERMINAL_RELEASED` is now load-bearing for the renderer too.** The flag at
  [`app/input_reader.rs:70-93`](../../../crates/cyrup-tui/src/app/input_reader.rs) already
  marks the two windows where the terminal belongs to a child (`suspend`,
  `edit_in_external_editor`). The render thread **must** consult it and paint nothing while it
  is set, or a frame lands in the middle of the user's `$EDITOR`.
- **`drain_and_restore` at shutdown** must stop the render thread and take the terminal back
  before restoring it — the same ordering the existing `stop_fullscreen(false)` +
  final-draw sequence at [`run.rs:405-420`](../../../crates/cyrup-tui/src/app/run.rs) gets
  for free today.
- **Panic safety.** [`panic_hook.rs`](../../../crates/cyrup-tui/src/panic_hook.rs) is
  process-global (`std::panic::set_hook`) so it already fires from any thread, and it closes
  the synchronized-update bracket **first** (`panic_hook.rs:56-70`) — which is exactly what a
  render thread dying mid-frame needs. It stays correct **provided the render thread holds no
  lock the restore needs**: keep it writing straight to `stdout`, never through a mutex the
  hook would have to take. Note release sets `panic = "abort"`, so no `Drop` can stand in.

### 3.4 Keep the escape hatch working, and extend it to the renderer

[`app/input_reader.rs:27-204`](../../../crates/cyrup-tui/src/app/input_reader.rs) is a
`std::thread` that hard-exits on three unserviced `Ctrl+C`/`Ctrl+D` chords and prints
``cyrup: run loop wedged in arm `{arm}` for {elapsed}`` from `ACTIVE_ARM` (`:111`). It is the
last line of defence and it must survive: `ArmGuard::enter("events")`
([`run_action.rs:293`](../../../crates/cyrup-tui/src/app/run_action.rs)) still needs to
bracket the fold, and the hatch must now also restore the terminal correctly when the
*render* thread is the one that is stuck.

Mirror the existing machinery rather than inventing a second one — the same
`Mutex<Option<(&'static str, Instant)>>` shape:

```rust
/// The frame currently being painted, and since when — written by the render thread, read by
/// the input reader's watchdog. Same shape and same `try_lock`-never-`lock` discipline as
/// `ACTIVE_ARM` (`input_reader.rs:111`), so a wedged renderer names itself instead of
/// presenting as a silent freeze.
pub(crate) static ACTIVE_FRAME: std::sync::Mutex<Option<std::time::Instant>> =
    std::sync::Mutex::new(None);
```

`hard_exit_from_reader` (`:194`) reports both. `mark_input_serviced` (`:52`) keeps meaning
"serviced" and stays after the frame request — a *requested* frame the user never sees is not
service, exactly as TUI-092 §7 property 7 already argues about a *drawn* one.

### 3.5 Delete what becomes dead, and say so

If the decoupling lands, the `biased;` ordering rationale at
[`run.rs:293-315`](../../../crates/cyrup-tui/src/app/run.rs) is no longer load-bearing for
*this* reason (it remains load-bearing for the cancel arm and the session-swap arm at
`:321-330`). **Rewrite that comment rather than leaving it to mislead** — it currently tells
the next reader that arm order is what protects the keyboard, and after this change it is
not.

Two more comments become false the moment §3.0 lands and must be corrected in the same
change, or the next reader will re-derive the wrong model of the render path:

* [`transcript/cache.rs:36-40`](../../../crates/cyrup-tui/src/transcript/cache.rs) — *"`markdown::render`
  emits ONE un-wrapped `Line` per prose paragraph"*. Already stale at HEAD (§0.2); after §3.0
  it is stale **and** describes deleted code.
* [`docs/gap-analysis/bugs/TUI-092-progressive-lockup.md`](../../../docs/gap-analysis/bugs/TUI-092-progressive-lockup.md)
  §7 property 2 — *"a frame with unchanged state is O(changed chrome)"*, with `cached_render`'s
  key check offered as *"the whole proof"*. It was never true; §3.0 is what makes it true.
  Update the ledger with the measurement, since that row is the reason nobody looked here.

---

## 4. Honest sizing — REVISED

The original sizing said *"the highest-risk task in the backlog and its benefit is the least
quantified"*, and recommended not starting until PERF-001 landed. **§0.2 changes both halves
of that**, but only for §3.0/§3.1.

| stage | risk | benefit | evidence |
| --- | --- | --- | --- |
| **§3.0** wrapped-row cache + windowed paint | **low** — one file's cache shape, no terminal, no threads | **122× at 16k rows; frame cost becomes constant in turn size** | measured, §0.2 / §6 |
| **§3.1** frame scheduler | **low** — one struct, one loop-body statement, 26 call-site edits | frames capped at 62.5 Hz; coalescing across arms, not just within one | pi parity, `tui.ts:343,772-822,896-900` |
| **§3.2** `FrameState` publish | **medium** — every chrome component gains a `lines()` seam | none on its own; a prerequisite | — |
| **§3.3** render thread | **high** — terminal ownership, shutdown ordering, panic safety, the escape hatch | only against a **blocking tty write**, which §3.0 cannot touch | argued, §1; not measured |

**Do §3.0 and §3.1 unconditionally. Then measure before committing to §3.2/§3.3.** The
exit criterion is in §5.4: if, after §3.0 and §3.1, a frame on a real terminal with a large
transcript is bounded and no longer scales with the turn, then the CPU justification for the
thread is spent and the remaining case rests entirely on tty blocking — which is a real
hazard, but a different one, and it deserves its own observation before it buys a
terminal-ownership refactor.

The original recommendation to land PERF-001 first still holds for §3.2/§3.3 and no longer
holds for §3.0: PERF-001 removes provider-side CPU from the same pipeline, but nothing in it
touches the redundant re-wrap, and §0.2's numbers were taken with the renderer in isolation.

Unlike [PERF-001](PERF-001_STREAM_SNAPSHOT_QUADRATIC.md), the *lockup* here is still
unobserved since TUI-092 round 2 (that doc's §8, Q2–Q5 open). If §3.3 is started, **get a
live reproduction first** (Q2: how many turns until spinner lag is noticeable?). §0.2 supplies
the prediction to test against: lag should begin around a few thousand active rows today and
not at all after §3.0.

---

## 5. Definition of Done

### 5.1 After §3.0 — frame cost

1. **A frame is constant in active-turn size.** The `Paragraph` built by
   `TranscriptView::render` receives at most `area.height` rows, carries no `.wrap()` and no
   `.scroll()`, and the transcript's rows reach it through `Arc::clone`, never `Vec::clone`.
   `grep -n 'lines.clone()\|\.wrap(Wrap' crates/cyrup-tui/src/transcript/cache.rs` returns
   nothing.
2. **Content is wrapped exactly once per materialisation.** `wrap_line` appears once on the
   live path (in `cached_render`) and once on the commit path (in `flush_committed`);
   `wrapped_height`'s `lines.to_vec()` measurement pass is gone.
3. **The viewport still sizes to wrapped rows.** A long single-paragraph streaming answer
   reserves its full wrapped height in the inline region and is not clipped to one row on
   commit — the PROSE-WRAP invariant `wrapped_height` existed for (R-ARCH-TUI-003/-005).
4. **OSC-8 links still land on the right text.** The href table is cached with the rows it was
   built from, and injection still runs after the blit.

### 5.2 After §3.1 — frame rate

5. **N state changes inside one 16 ms window produce one frame.** The only unconditional
   production paint is the top-of-body site in `App::run`; every arm requests.
6. **A keystroke is never delayed by the throttle.** The input arm forces an immediate frame
   (pi `tui.ts:896-900`), and `mark_input_serviced` still fires after it.
7. **Frames coalesce at least as well as F3 does today.** N queued deltas produce ≤1 frame.
   The structural guard in
   [`src/tests/run_loop_draw_coalescing.rs`](../../../crates/cyrup-tui/src/tests/run_loop_draw_coalescing.rs)
   counts `draw_synchronized()` per arm body, so it must be **re-anchored to the request
   sites, not deleted** — same for
   [`run_loop_input_priority.rs`](../../../crates/cyrup-tui/src/tests/run_loop_input_priority.rs)
   and [`render_cache_tick.rs`](../../../crates/cyrup-tui/src/tests/render_cache_tick.rs),
   whose `bump_render_tick`-before-draw ordering becomes bump-before-*request*.
8. **The five synchronous sites remain synchronous.** Seed frame (`run.rs:120`), the final
   frame on the exit path (`run.rs:419`), and the three terminal-handed-back redraws
   (`crossterm.rs:136`/`:150`/`:170`) — pi's `renderNow`.

### 5.3 After §3.2/§3.3 — the decoupling

9. **Input is never starved by draw cost.** With an artificially slowed frame (say 500 ms),
   keystrokes are still serviced promptly and the editor stays responsive. This is the
   property that must hold *structurally* — not "does not reproduce", but "cannot".
10. **The agent is not throttled by the renderer.** With the same artificially slowed frame,
    `Fanout::emit`'s awaited sends do not stall the provider stream: token throughput is
    unchanged from a fast-frame baseline.
11. **Terminal state is correct on every exit path.** Normal quit, `Ctrl+D`, the double-tap
    `Ctrl+C`, the three-chord hard exit from the reader thread, a panic on the run-loop task,
    and a panic on the **render thread** each leave a usable shell out of raw mode and out of
    the alternate screen.
12. **The wedge detector still reports.** A stuck fold still produces
    ``cyrup: run loop wedged in arm `{arm}` for {elapsed}``, and a stuck *render thread* is
    also detected and reported rather than appearing as a silent freeze.
13. **No tokio worker does terminal I/O.** The render thread is an OS thread.
14. **The renderer paints nothing while `TERMINAL_RELEASED` is set** — no frame lands inside
    `$EDITOR` or across a `Ctrl+Z` suspend.
15. **Alternate-screen transitions are ordered against frames.** Entering and leaving the
    alternate screen, resizes, and `append_lines` anchoring produce no torn or misplaced
    output under a streaming turn.

### 5.4 The gate, and the go/no-go for §3.3

16. **The suite is green under the real gate:**
    `cargo test --workspace --features test-fixtures --no-fail-fast`, and
    `cargo clippy --workspace --all-targets --features test-fixtures` exits **0** (the
    no-panic lints — `unwrap_used`, `expect_used`, `panic`, `indexing_slicing` — do **not**
    fire under `cargo build`/`cargo test`; check the exit code, not the output).
17. **A live terminal check, not just `TestBackend`.** Per the workspace rule this doc's
    predecessor states explicitly: a TUI claim is not settled by `TestBackend`. Drive a real
    session with a large transcript and confirm 1, 5, 6 and 11 by hand.
18. **The go/no-go.** On that live session, with the largest transcript reachable, record
    whether the spinner still visibly slips. If it does not, §3.3 is not justified by CPU and
    the remaining case for it is tty blocking alone — record that finding here and decide
    deliberately rather than by momentum.

---

## 6. [AUG] The probe

Standalone, no workspace edit, `--release`. Reproduces §0.2's table.

```rust
// tmp/perf005-probe/src/main.rs   (Cargo.toml: ratatui = "0.30.2", plus an empty [workspace])
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget, Wrap};
use std::borrow::Cow;
use std::time::Instant;

fn build(rows: usize) -> Vec<Line<'static>> {
    (0..rows).map(|r| Line::from((0..8).map(|s| Span::styled(
        format!("{:12}", format!("r{r}s{s}")),
        Style::default().fg(Color::Rgb(200,180,90)).add_modifier(Modifier::BOLD))).collect::<Vec<_>>())).collect()
}
/// The cheap alternative to `Vec::clone()`: same tree, every span a `Cow::Borrowed`.
fn shallow<'a>(src: &'a [Line<'static>]) -> Vec<Line<'a>> {
    src.iter().map(|l| Line {
        spans: l.spans.iter().map(|s| Span { content: Cow::Borrowed(s.content.as_ref()), style: s.style }).collect(),
        style: l.style, alignment: l.alignment }).collect()
}
fn timed(iters: usize, mut f: impl FnMut()) -> f64 {
    for _ in 0..(iters/10).max(1) { f(); }
    let t = Instant::now();
    for _ in 0..iters { f(); }
    t.elapsed().as_secs_f64() * 1e6 / iters as f64
}
fn main() {
    let area = Rect::new(0, 0, 100, 30);
    let h = area.height as usize;
    for (rows, it) in [(20usize,3000usize),(200,2000),(1_000,800),(2_000,400),(4_000,200),(8_000,100),(16_000,60)] {
        let src = build(rows);
        let scroll = rows.saturating_sub(h).min(u16::MAX as usize) as u16;
        let mut buf = Buffer::empty(area);
        // A — today: deep clone + full re-wrap + scroll
        let a = timed(it, || { buf.reset();
            Paragraph::new(src.clone()).wrap(Wrap{trim:false}).scroll((scroll,0)).render(area,&mut buf); });
        // B — drop the deep clone only
        let b = timed(it, || { buf.reset();
            Paragraph::new(shallow(&src)).wrap(Wrap{trim:false}).scroll((scroll,0)).render(area,&mut buf); });
        // C — cache holds already-wrapped rows: drop `.wrap()`
        let c = timed(it, || { buf.reset();
            Paragraph::new(shallow(&src)).scroll((scroll,0)).render(area,&mut buf); });
        // D — §3.0: window the paint to the visible rows
        let d = timed(it, || { buf.reset();
            let first = rows.saturating_sub(h).min(rows);
            Paragraph::new(shallow(src.get(first..).unwrap_or(&[]))).render(area,&mut buf); });
        println!("| {rows} | {a:.0} µs | {b:.0} µs | {c:.0} µs | {d:.0} µs |");
    }
}
```

Reference run on this host (2026-08-29, `--release`, 100 × 30 area) — §0.2's table.
Companion figures from the same probe:

* `Vec<Line>::clone()` alone: `7 µs` @20 rows · `567 µs` @1k · `2 452 µs` @4k · `12 081 µs` @16k.
* `Arc<Vec<Line>>::clone()`: **`11 ns`, flat** at every size — the §3.0/§3.2 publish.
* Deep clone against the 80 ms spinner tick: `23.2 ms` @30k rows (29%) · `47.7 ms` @60k (60%) ·
  `95.1 ms` @120k (119%). The full frame crosses the tick far earlier, at ~14k rows.

**Two traps.** Measure in `--release`: a debug build's allocator noise swamps the C→D
difference. And measure the *frame*, not the clone — the clone is only ~4% of it, and a probe
that times `lines.clone()` alone reports the least important of the three costs as if it were
the finding.
