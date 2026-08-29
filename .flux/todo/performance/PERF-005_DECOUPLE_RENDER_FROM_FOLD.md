---
stage: new
status: pending
updated: 2026-08-29 02:33
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

---

## 0. READ THIS FIRST — do not re-do TUI-092

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
mode is mitigated, not eliminated — the `biased;` ordering comment at `run.rs:293-301`
exists precisely because it is still reachable in principle.

Also from that doc's §8: **the original defect has not been re-observed since round 2
landed, and nobody has confirmed the fix live** — the workspace's standing rule is that a
TUI claim is not settled by `TestBackend`. So this task is a *structural* hardening of a
path whose current real-world behaviour is unmeasured. Size it accordingly, and see §4.

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

`draw_synchronized` is [`app/crossterm.rs:87`](../../../crates/cyrup-tui/src/app/crossterm.rs),
and there are ~40 call sites across `run_arms.rs` (22), `run.rs` (4), `run_action.rs` (2)
and `draw.rs`/`crossterm.rs`. Every one of them is on the run-loop task.

The consequence chain:

1. Draw cost grows with active-turn size (bounded by `RenderCache`, but the *cache miss*
   still costs a full markdown + syntect pass, and a text delta bumps the generation —
   [`transcript/cache.rs:5-11,28-53`](../../../crates/cyrup-tui/src/transcript/cache.rs)).
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

---

## 2. What pi cannot do, and why that matters here

pi has one event-loop thread. `Promise.all` is concurrency, not parallelism; its renderer,
its fold, and its tool execution all contend for the same thread by construction. There is
no version of this fix available to it.

cyrup can put the renderer on its own thread and make input starvation *structurally
impossible* rather than merely unlikely. That is the "leave pi in the dust" content of this
task — not a constant-factor win, a different failure surface.

---

## 3. Required implementation

### 3a. Split the state the renderer reads from the state the loop mutates

The renderer needs a consistent snapshot, not a lock on live `AppState`. Introduce a
double-buffered frame model:

```rust
/// What one frame needs, produced by the fold and consumed by the render thread. Owns its
/// data outright: the render thread must never hold a borrow into `AppState`, or the fold
/// blocks on it and the decoupling buys nothing.
struct FrameState { /* transcript lines, chrome, layout inputs, theme generation, … */ }
```

Publish with `arc_swap::ArcSwap<FrameState>` or a `watch::channel<Arc<FrameState>>`:
the fold does `store(Arc::new(next))` (wait-free), the renderer does `load()`. A frame the
renderer misses is a frame it did not need to draw — **coalescing for free**, which is F3's
guarantee upheld structurally rather than by an explicit drain.

**`FrameState` must be cheap to build**, or the cost simply moves from draw to fold and
nothing is gained. This is the crux of the task. Reuse the existing `RenderCache` — the
lines are already materialised and cached by `(generation, width, theme_generation)`; the
frame publish should be an `Arc` clone of that cache, not a rebuild.

### 3b. Run the terminal writes on a dedicated thread at a fixed cadence

A `std::thread` (not a tokio task — it does blocking terminal I/O and must not occupy a
runtime worker) that wakes on a ~60 Hz timer or on a publish notification, loads the current
`Arc<FrameState>`, and draws. Terminal ownership moves entirely to this thread; nothing else
may write to the terminal concurrently.

Sequencing hazards that must be handled explicitly:

- **Alternate-screen enter/leave, raw-mode toggles, and terminal restore** must be ordered
  against frames. Today they are trivially ordered because everything is on one task. See
  [`app/crossterm.rs:110-112,200`](../../../crates/cyrup-tui/src/app/crossterm.rs) (the
  by-design block announced to the wedge detector) and
  [`app/backend.rs`](../../../crates/cyrup-tui/src/app/backend.rs)'s `append_lines`
  anchoring, which depends on knowing exactly what was drawn last.
- **`drain_and_restore` at shutdown** must stop the render thread and take the terminal back
  before restoring it — the same ordering the existing `stop_fullscreen(false)` +
  final-draw sequence at [`run.rs:405-420`](../../../crates/cyrup-tui/src/app/run.rs) gets
  for free today.
- **Panic safety.** A panic on the render thread must not leave the terminal in raw mode.
  [`panic_hook.rs`](../../../crates/cyrup-tui/src/panic_hook.rs) already exists for the
  single-threaded case and needs to cover the new thread.

### 3c. Keep the escape hatch working

[`app/input_reader.rs:27-204`](../../../crates/cyrup-tui/src/app/input_reader.rs) is a
`std::thread` that hard-exits on three unserviced `Ctrl+C`/`Ctrl+D` chords and prints
``cyrup: run loop wedged in arm `{arm}` for {elapsed}`` from `ACTIVE_ARM`. It is the last
line of defence and it must survive: `ArmGuard::enter("events")`
([`run_action.rs:293`](../../../crates/cyrup-tui/src/app/run_action.rs)) still needs to
bracket the fold, and the hatch must now also restore the terminal correctly when the
*render* thread is the one that is stuck.

### 3d. Delete what becomes dead, and say so

If the decoupling lands, the `biased;` ordering rationale at
[`run.rs:293-315`](../../../crates/cyrup-tui/src/app/run.rs) is no longer load-bearing for
*this* reason (it remains load-bearing for the cancel arm and the session-swap arm at
`:321-330`). **Rewrite that comment rather than leaving it to mislead** — it currently tells
the next reader that arm order is what protects the keyboard, and after this change it is
not.

---

## 4. Honest sizing — read before prioritising

**This is the highest-risk task in the backlog and its benefit is the least quantified.**

- The failure it prevents has **not been observed since TUI-092 round 2 landed** (that
  doc's §8, Q2–Q5 still open). It may already be unreachable in practice.
- It touches terminal ownership, shutdown ordering, panic safety and the escape hatch — the
  four things that turn a performance change into an unusable terminal.
- Unlike [PERF-001](PERF-001_STREAM_SNAPSHOT_QUADRATIC.md), there is no measurement here
  saying "2,883×". There is a structural argument and an in-code warning.

**Recommendation: do not start this until PERF-001 has landed and been measured.** PERF-001
removes work from the same pipeline; if the provider stops burning seconds of CPU per tool
call, the fold gets faster, `Fanout`'s backpressure eases, and the draw budget widens. It is
entirely possible that after PERF-001 there is no observable render pressure left to fix —
and finding that out costs one measurement instead of a terminal-ownership refactor.

If it is started, **get a live reproduction first** (TUI-092 §8 Q2: how many turns until
spinner lag is noticeable?). Without one there is no way to tell whether the change helped.

---

## 5. Definition of Done

1. **Input is never starved by draw cost.** With an artificially slowed frame (say 500 ms),
   keystrokes are still serviced promptly and the editor stays responsive. This is the
   property that must hold *structurally* — not "does not reproduce", but "cannot".
2. **The agent is not throttled by the renderer.** With the same artificially slowed frame,
   `Fanout::emit`'s awaited sends do not stall the provider stream: token throughput is
   unchanged from a fast-frame baseline.
3. **Frames coalesce at least as well as F3 does today.** N queued deltas produce ≤1 frame,
   verified against the existing structural guard in
   [`src/tests/run_loop_draw_coalescing.rs`](../../../crates/cyrup-tui/src/tests/run_loop_draw_coalescing.rs)
   — that test must still pass or be replaced with an equivalent that pins the new shape.
4. **Terminal state is correct on every exit path.** Normal quit, `Ctrl+D`, the double-tap
   `Ctrl+C`, the three-chord hard exit from the reader thread, a panic on the run-loop task,
   and a panic on the **render thread** each leave a usable shell out of raw mode and out of
   the alternate screen.
5. **The wedge detector still reports.** A stuck fold still produces
   ``cyrup: run loop wedged in arm `{arm}` for {elapsed}``, and a stuck *render thread* is
   also detected and reported rather than appearing as a silent freeze.
6. **No tokio worker does terminal I/O.** The render thread is an OS thread.
7. **Alternate-screen transitions are ordered against frames.** Entering and leaving the
   alternate screen, resizes, and `append_lines` anchoring produce no torn or misplaced
   output under a streaming turn.
8. **The suite is green under the real gate:**
   `cargo test --workspace --features test-fixtures --no-fail-fast`, and
   `cargo clippy --workspace --all-targets --features test-fixtures` exits **0**.
9. **A live terminal check, not just `TestBackend`.** Per the workspace rule this doc's
   predecessor states explicitly: a TUI claim is not settled by `TestBackend`. Drive a real
   session with a large transcript and confirm 1, 2 and 4 by hand.
