# TUI-092 — The TUI degrades over the life of a session and ends in a total lockup

> **Status** — **OPEN, round 2 — AUDIT COMPLETE, FIX SPLIT INTO EIGHT DISCRETE TASKS.** Round 1
> landed on 2026-08-15 and measurably improved the app; it did not eliminate the defect. Round 2's
> audit (§5) is **done**: every collection, cache, buffer, channel, task, subscription and
> terminal-side write path in `crates/cyrup-tui` has been classified against the owner's bar, and the
> eight defects that violate it are specified with their fixes.
>
> **This file is now the umbrella/index.** The eight defects are broken out into one independently
> resolvable task file each, listed in §6 in the order they should land. Each child file is
> self-contained (evidence + prescriptive fix + call-site changes + its own definition of done) and
> links back here for the shared picture. The per-defect detail that used to live in §5.1–§5.8 has
> **moved** to those children; this file keeps the shared context (§1–§4), the audit verification
> trail (§5.0), the cleared list (§5.9), the aggregate definition of done (§7), the open questions
> (§8) and the cross-references (§9).
>
> **Kind** `cyrup-original` · **Severity** **high** *(pending the escape-hatch answer in §8 — if a
> locked-up session cannot be exited, this is **critical**)* · **Effort** **M**

**Reported from live use** by the owner, 2026-08-15, after round 1 shipped.

---

## 1. The report, verbatim

> *"it's much better so the fix did improve things but slowly over time you can see the spinner start
> to lag, all rendering begins to lag and eventually the whole terminal locks up"*

---

## 2. What round 1 changed, and why that matters here

Round 1 is landed, QA-reviewed at 9/10, and closed three mechanisms:

* **Input-arm starvation** — `input.next()` was arm #7 in a `biased;` `select!`, below an 80 ms
  spinner ticker that is armed for the whole of a streaming turn. Once a frame cost more than a
  tick, the input arm was never polled again. It is now arm #2, directly under the cancel arm.
* **`drain_queue` self-deadlock** — `Escape` / `Alt+Up` / `/tree` awaited a session call that ends in
  an awaited send into the run loop's own bounded `events` channel. Moved off the loop task.
* **`execute_command`'s session-lifecycle arms** — `/new`, `/reload`, `/import`, `/resume`, `/fork`
  awaited runtime ops that can re-enter the loop through a guest `ui.*` dialog. Moved off the loop
  task.

Round 1 also added an unblockable escape hatch in the input reader thread (three unserviced
`Ctrl+C`/`Ctrl+D` chords → restore the terminal and exit 130).

**The significance for round 2 is what this rules out.** The owner confirms the app is *better*, so
the input path is no longer the bottleneck: keystrokes are being serviced, and the loop is still
iterating. The remaining defect therefore is **not** a starved input arm and **not** one of the three
closed deadlocks. Something else grows over the life of a session until the app stops working.

---

## 3. The observed behaviour, in the order it appears

The failure is **progressive and ordered**, and the ordering is the most diagnostic thing in the
report. It is not a single stall.

| Phase | What the user sees |
| --- | --- |
| 1 — fresh session | Fast and smooth. Nothing wrong. |
| 2 — after a while | **The spinner starts to lag.** The Braille frame stops advancing at its usual cadence. |
| 3 — later | **All rendering begins to lag.** Not just the spinner — the whole UI paints late. |
| 4 — eventually | **The whole terminal locks up.** |

Three things to hold onto:

* **The spinner is the canary.** It ticks on a fixed 80 ms interval and is *only* a redraw. When it
  visibly slips, one frame is already costing more than 80 ms — before the user notices anything
  else. Whatever this is, it shows up in per-frame cost first.
* **Degradation is global, not localised.** The owner says *all* rendering lags, not "long replies
  lag" or "tool output lags". Everything gets slower together.
* **It ends in a lockup, not an error.** No panic, no message, no crash — it stops. A slow app stays
  responsive-but-late; a locked-up one does not. Something crosses from *slow* to *stopped*.

**It correlates with session age, not with any one action.** The owner's word is "slowly over
time" — this accumulates across a session rather than firing on a particular command or a particular
kind of content.

---

## 4. What this has to be — and what the audit found it to be

Stated as a class, because that is the only shape consistent with all four phases:

> **Something in the TUI grows without bound as a session runs, and the app pays for it on every
> frame.**

The audit is now complete, and the answer is **not one culprit — it is eight, and they compound**:

| # | Defect | Cost shape | Phase it drives | Task file |
| --- | --- | --- | --- | --- |
| F1 | `AppState::scrollback` — every committed line cloned into a test-only accumulator, retained for the process lifetime, never cleared on session swap | memory ∝ **total session output** | 3→4 (swap pressure; the only structure that grows with *session age*) | [TUI-092-F1](TUI-092-F1-scrollback-accumulator.md) |
| F2 | The active region is materialised **3× per frame** — full markdown parse + syntect highlight + image rasterisation of the whole streaming turn, plus two full `Vec<Line>` clones for wrap measurement | CPU/frame ∝ **active turn size** | 2→3 (the spinner canary: one frame crosses 80 ms) | [TUI-092-F2](TUI-092-F2-transcript-render-cache.md) |
| F3 | **No draw coalescing** — one full frame per session event, per bash chunk, per keystroke | frames/s ∝ event rate; with F2, a turn costs O(turn²) | 2→3 | [TUI-092-F3](TUI-092-F3-draw-coalescing.md) |
| F4 | `refresh_context_usage` rebuilds the **entire branch message list** (with clones) on every `MessageEnd`/`AgentEnd`, awaited on the run-loop task | CPU/event ∝ **session history** | 3 (every turn's frame stalls a little more) | [TUI-092-F4](TUI-092-F4-context-usage-reverse-scan.md) |
| F5 | ratatui `scrolling-regions` is **off**: every commit flush ends in `Terminal::clear()` → the next frame is a **full viewport repaint**, not a cell diff | bytes/frame spike per commit | 2 (commit cadence during a turn) | [TUI-092-F5](TUI-092-F5-scrolling-regions.md) |
| F6 | `BashExecution::output_lines` accumulates **every** output line of a live `!`/`!!` run; the session-side sink forwards every chunk uncapped | memory ∝ run output | 2→3 during chatty runs | [TUI-092-F6](TUI-092-F6-bash-output-ring.md) |
| F7 | `ImageRenderer::render` re-encodes the image protocol (raster clone + resize + base64) **every frame** per attached image | CPU/frame ∝ attached image px | 2 while attachments sit | [TUI-092-F7](TUI-092-F7-image-renderer-protocol-cache.md) |
| F8 | The run loop's event ingest **clones** `args` / `partial_result` / `result` / queue vectors per event instead of moving them | CPU/event ∝ payload size | 3 | [TUI-092-F8](TUI-092-F8-by-value-ingest.md) |

The compounding is the lockup: F2 makes frames expensive; F3 multiplies expensive frames by event
rate; F5 makes every commit frame a full repaint; F4 makes the stall grow with every turn; F1 grows
memory until the allocator and the OS compressor join in; and once frame cost × event rate > 1 the
loop falls permanently behind — the de facto lockup of phase 4, reached with no error and no panic,
exactly as reported.

---

## 5. The audit, completed — with the fix for each finding

The owner's requirement, and it is broader than "make the lockup go away":

> **Everything in the terminal that leaks or is unbounded has to be fixed. It has to be elite and
> tight.**

So round 2 is not "find the one leak". It is an **audit with a bar**: every collection, cache,
buffer, channel, task, subscription and terminal-side write path in `crates/cyrup-tui` is either

* provably **bounded** (a cap, an eviction policy, or a lifetime that ends with the turn), or
* provably **O(1) per frame** and never walked over history,

and anything that is neither is a defect under this row, whether or not it is *the* cause of the
lockup. **A session that runs for hours must cost what a fresh one costs.**

The eight defects are in §6 (one task file each). §5.9 is the list of structures that were audited
and **cleared** — do not re-audit them.

---

### §5.0 — Audit verification (read in the tree, not assumed)

Every finding was confirmed against the working tree before the task files were written. The line
numbers are the **current** line numbers; the handful that drifted from earlier drafts are
corrected inline in each child file so a developer lands each fix on a real anchor, not a
remembered one. This table is the overview; each child file carries its own verified anchors.

| # | Anchor read | Confirmation |
| --- | --- | --- |
| F1 | field [`app.rs:459`](../../../crates/cyrup-tui/src/app.rs#L459); extend [`app.rs:2100`](../../../crates/cyrup-tui/src/app.rs#L2100); accessors [`app.rs:1374`](../../../crates/cyrup-tui/src/app.rs#L1374)/[`:1388`](../../../crates/cyrup-tui/src/app.rs#L1388) | `rebind_session` at [`app.rs:1617`](../../../crates/cyrup-tui/src/app.rs#L1617) resets transcript/queues/selector/overlays/indicator and **never touches `scrollback`**. The only two non-`src/tests/` in-crate accessor call sites are [`app.rs:10195`](../../../crates/cyrup-tui/src/app.rs#L10195) and [`app.rs:10234`](../../../crates/cyrup-tui/src/app.rs#L10234), both inside `#[cfg(test)]` modules (boundaries [`:10126`](../../../crates/cyrup-tui/src/app.rs#L10126) and [`:10273`](../../../crates/cyrup-tui/src/app.rs#L10273)). The external consumer is [`cyrup-it/tests/bin/wasm_renderer_screen.rs:119`](../../../crates/cyrup-it/tests/bin/wasm_renderer_screen.rs#L119)/[`:144`](../../../crates/cyrup-it/tests/bin/wasm_renderer_screen.rs#L144); its crate pins `cyrup-tui = { workspace = true }` at [`cyrup-it/Cargo.toml:95`](../../../crates/cyrup-it/Cargo.toml#L95) — the feature goes there. |
| F2 | `content_height(&self)` [`transcript.rs:1137`](../../../crates/cyrup-tui/src/transcript.rs#L1137) → `lines(&self)` [`:1147`](../../../crates/cyrup-tui/src/transcript.rs#L1147); `region_constraints(&AppState)` [`app.rs:7495`](../../../crates/cyrup-tui/src/app.rs#L7495); `content_height` call [`:7612`](../../../crates/cyrup-tui/src/app.rs#L7612); `live_region_height(&AppState)` [`:7632`](../../../crates/cyrup-tui/src/app.rs#L7632); `render` re-calls `region_constraints` [`:7646`](../../../crates/cyrup-tui/src/app.rs#L7646); `TranscriptView::render` calls `self.lines()` a 3rd time [`transcript.rs:3254`](../../../crates/cyrup-tui/src/transcript.rs#L3254) | **The `&mut` propagation is borrow-clean.** `TranscriptView::render` is already `&mut self` ([`transcript.rs:3253`](../../../crates/cyrup-tui/src/transcript.rs#L3253)); `render` owns `state: &mut AppState` ([`app.rs:7643`](../../../crates/cyrup-tui/src/app.rs#L7643)); `draw` calls `live_region_height` at [`app.rs:1986`](../../../crates/cyrup-tui/src/app.rs#L1986) **before** destructuring `self` into `terminal`/`state` at [`app.rs:2023`](../../../crates/cyrup-tui/src/app.rs#L2023) and handing `state` to `render` via `terminal.draw(|frame| render(frame, state))` at [`app.rs:2024`](../../../crates/cyrup-tui/src/app.rs#L2024); `App::new` calls `live_region_height` on an owned local `state` at [`app.rs:1119`](../../../crates/cyrup-tui/src/app.rs#L1119). All four call sites supply `&mut` with no borrow conflict. |
| F3 | `events` arm [`app.rs:8846`](../../../crates/cyrup-tui/src/app.rs#L8846); `input` arm [`app.rs:8423`](../../../crates/cyrup-tui/src/app.rs#L8423); `bash_next` arm [`app.rs:8638`](../../../crates/cyrup-tui/src/app.rs#L8638) | `now_or_never` is `futures::FutureExt::now_or_never`; `futures = { version = "0.3" }` is the workspace dep at [`Cargo.toml:122`](../../../Cargo.toml#L122) and already a direct dep of `cyrup-tui` — **no new dependency**. The reader thread's channel is `unbounded_channel::<InputEvent>()` at [`app.rs:9445`](../../../crates/cyrup-tui/src/app.rs#L9445) (a `std::thread` that cannot `.await`) and stays unbounded by design; the `ArmGuard::enter("events")` span at [`app.rs:8849`](../../../crates/cyrup-tui/src/app.rs#L8849) is preserved verbatim inside the drained arm. |
| F4 | `context_usage` [`session.rs:4117`](../../../crates/cyrup-session-svc/src/session.rs#L4117) → `messages()` [`:3929`](../../../crates/cyrup-session-svc/src/session.rs#L3929); `build_context` [`manager.rs:737`](../../../crates/cyrup-session/src/manager.rs#L737); `branch_path` [`:627`](../../../crates/cyrup-session/src/manager.rs#L627); `from_last_assistant` [`state.rs:285`](../../../crates/cyrup-session-svc/src/state.rs#L285) | **Correction:** `build_context_messages` is *defined* in [`cyrup-session/src/context.rs:151`](../../../crates/cyrup-session/src/context.rs#L151) (called from [`manager.rs:766`](../../../crates/cyrup-session/src/manager.rs#L766)), not `manager.rs`. `has_post_compaction_usage` at [`session.rs:4080`](../../../crates/cyrup-session-svc/src/session.rs#L4080) already walks `entries()` clone-free. `Message` is already in scope at the top of [`session.rs:13`](../../../crates/cyrup-session-svc/src/session.rs#L13), so the rewritten body needs only the two `cyrup_session` imports shown — no new top-level `use`. |
| F5 | feature line [`cyrup-tui/Cargo.toml:50`](../../../crates/cyrup-tui/Cargo.toml#L50); dispatch [`tmp/ratatui-core-0.1.2/src/terminal/inline.rs:113`](../../../tmp/ratatui-core-0.1.2/src/terminal/inline.rs#L113) | **Correction:** the no-regions `self.clear()?` is at [`inline.rs:212`](../../../tmp/ratatui-core-0.1.2/src/terminal/inline.rs#L212) (tmux-workaround comment [`:210`](../../../tmp/ratatui-core-0.1.2/src/terminal/inline.rs#L210)–`211`); the scrolling-regions path starts at [`inline.rs:228`](../../../tmp/ratatui-core-0.1.2/src/terminal/inline.rs#L228); the crossterm `scroll_region_up`/`down` impls at [`tmp/ratatui-crossterm-0.1.2/src/lib.rs:362`](../../../tmp/ratatui-crossterm-0.1.2/src/lib.rs#L362)–`383`. |
| F6 | `output_lines: Vec<String>` [`bash.rs:45`](../../../crates/cyrup-tui/src/bash.rs#L45); `append_output` [`:121`](../../../crates/cyrup-tui/src/bash.rs#L121); `DEFAULT_MAX_LINES = 2000` [`cyrup-tools/src/truncate.rs:11`](../../../crates/cyrup-tools/src/truncate.rs#L11) | The session-side sink forwards every chunk; the rolling 100 KB cap applies to the result preview only. |
| F7 | `ImageRenderer { picker }` [`image.rs:34`](../../../crates/cyrup-tui/src/image.rs#L34); `render(&self)` [`:152`](../../../crates/cyrup-tui/src/image.rs#L152); per-frame `new_protocol(block.image.clone(), …)` [`:165`](../../../crates/cyrup-tui/src/image.rs#L165); `render_images` [`app.rs:7866`](../../../crates/cyrup-tui/src/app.rs#L7866); `pending_images.clear()` [`app.rs:1462`](../../../crates/cyrup-tui/src/app.rs#L1462) | `Picker::new_protocol` returns an owned, reusable `Protocol` ([`tmp/ratatui-image-11.0.6/src/picker.rs:256`](../../../tmp/ratatui-image-11.0.6/src/picker.rs#L256)) — caching it is the library's own `StatefulImage` pattern. `render` is `&self`, so the cache uses interior mutability. |
| F8 | clones [`app.rs:6005`](../../../crates/cyrup-tui/src/app.rs#L6005)/[`:6027`](../../../crates/cyrup-tui/src/app.rs#L6027)/[`:6037`](../../../crates/cyrup-tui/src/app.rs#L6037)/[`:6054`](../../../crates/cyrup-tui/src/app.rs#L6054); borrow `ev: &AgentSessionEvent` [`app.rs:5871`](../../../crates/cyrup-tui/src/app.rs#L5871) | The transcript APIs already consume by value: `push_tool_start_rendered(…, args: Value, …)` [`transcript.rs:731`](../../../crates/cyrup-tui/src/transcript.rs#L731), `push_tool_update(…, partial: Option<Value>)` [`:760`](../../../crates/cyrup-tui/src/transcript.rs#L760), `push_tool_end_rendered(…, result: Option<Value>, …)` [`:826`](../../../crates/cyrup-tui/src/transcript.rs#L826). The clones exist **only** because the ingest path borrows `&ev`. |

**Hyperlink convention:** every path is relative to this directory (`docs/gap-analysis/bugs/`),
so `../../../crates/…` resolves into the workspace root and `../../../tmp/…` into the vendored
sources used for the F5/F7 citations. The child task files live in this same directory and use the
same convention.

---

### §5.9 — Audited and CLEARED (do not re-audit)

These were traced to their bounds and are **not** defects under the bar:

| Structure | Bound |
| --- | --- |
| Editor undo stack | capped at 500 ([`editor.rs:845`](../../../crates/cyrup-tui/src/editor.rs#L845)) |
| Editor prompt history | capped at 100 ([`editor.rs:7`](../../../crates/cyrup-tui/src/editor.rs#L7)) |
| `TranscriptView::pending` | drained every frame by [`drain_committed`](../../../crates/cyrup-tui/src/transcript.rs#L553) (`mem::take`) |
| `active_tools` / `streaming` / `thinking` / `bash` | committed at turn end; finished tools flushed mid-turn ([`commit_finished_leading_tools`](../../../crates/cyrup-tui/src/transcript.rs#L926)) |
| Session event `Fanout` channels | bounded (`CHANNEL_CAPACITY = 1024`), closed senders pruned per emit ([`subscriber.rs:64-76`](../../../crates/cyrup-session-svc/src/subscriber.rs#L64)) |
| Run-scoped subscriptions | cleared on settle (`Fanout::end_run`); persistent ones dropped on swap; `invalidate` clears both |
| Tool-result image rasters | decoded **once** at `ToolExecutionEnd`, downscaled to [`MAX_RASTER_PX` = 1024](../../../crates/cyrup-tui/src/transcript.rs#L1263) |
| Bash result preview (session side) | rolling 100 KB cap + temp-file spill ([`cyrup-session-svc/src/bash.rs:293-330`](../../../crates/cyrup-session-svc/src/bash.rs#L293)) |
| `pending_images` (attachments) | cleared on submit ([`app.rs:1462`](../../../crates/cyrup-tui/src/app.rs#L1462)) |
| `EscapeReassembler` / `StrayReplyFilter` held buffers | flushed on every input-idle tick ([`app.rs:9488-9495`](../../../crates/cyrup-tui/src/app.rs#L9488)) |
| `extension_statuses` | `BTreeMap` keyed by extension id; blank values remove entries |
| `session_queue` / `pending_messages` / `compaction_queue` | replaced per `queue_update`; cleared on session swap |
| ratatui `Terminal` buffers | sized to the viewport; rebuilt on resize; `insert_before` retains nothing (verified in [`ratatui-core inline.rs`](../../../tmp/ratatui-core-0.1.2/src/terminal/inline.rs#L130)) |
| Syntect `SyntaxSet` | process-wide `OnceLock`, built once ([`markdown.rs:1742`](../../../crates/cyrup-tui/src/markdown.rs#L1742)) |
| Spinner/dialog/progress/git/elapsed tickers | `MissedTickBehavior::Skip`, `if`-gated, idempotent |
| Per-submission / per-shortcut spawned tasks | end with their op; channels dropped with them |
| `App::live_floor` / `viewport_height` | scalars; the TUI-090 floor release is preserved untouched |

The one structural fact that makes the cleared list possible: committed history leaves the process
entirely — it is written to the terminal's native scrollback exactly once
([`flush_committed`](../../../crates/cyrup-tui/src/app.rs#L2056)) and never re-rendered. What the
emulator itself retains is terminal-side state, outside this process's reach; the app's obligation
under the bar is to write each line once and keep nothing — which, after F1, it does.

---

## 6. Implementation plan (ordered) — one task file per defect

Land in this order; each step is independently correct and independently shippable, so each is its
own task file. Two pairs coordinate by a single call-site choice but still ship independently (noted
inline).

| # | Task file | What it does | Coordinates with |
| --- | --- | --- | --- |
| 1 | [**F5** — `scrolling-regions`](TUI-092-F5-scrolling-regions.md) | one-line `Cargo.toml` feature flag; kills the per-commit full repaint | nothing |
| 2 | [**F1** — `scrollback-accumulator`](TUI-092-F1-scrollback-accumulator.md) | remove the test-only accumulator from production at compile time via a test-only cargo feature (production never compiles it; tests keep it via `#[cfg(any(test, feature = …))]`); kills session-age memory growth | nothing |
| 3 | [**F2** — `TranscriptView` render cache](TUI-092-F2-transcript-render-cache.md) | generation counter + `RenderCache` + `cached_render`; `&mut` propagation through `region_constraints`/`live_region_height`; the full mutator-bump list. The big one | nothing (borrow-clean) |
| 4 | [**F3** — drain-then-draw](TUI-092-F3-draw-coalescing.md) | the `events`, `input`, `bash_next` arms each drain every ready message, then draw once | **F8** (one call-site choice; either order) |
| 5 | [**F8** — by-value ingest](TUI-092-F8-by-value-ingest.md) | `ingest_event_rendered_owned` + reference wrappers for tests; moves, not clones | **F3** (one call-site choice; either order) |
| 6 | [**F4** — `context_usage` reverse scan](TUI-092-F4-context-usage-reverse-scan.md) | one lock + one reverse branch walk, zero message clones, in `cyrup-session-svc` | nothing |
| 7 | [**F6** — bash output ring](TUI-092-F6-bash-output-ring.md) | `VecDeque` + 2000-line cap + omission counter + one dim render row | nothing |
| 8 | [**F7** — `ImageRenderer` protocol cache](TUI-092-F7-image-renderer-protocol-cache.md) | memoise the built `Protocol` inside `ImageRenderer` (interior mutability) | nothing |

**Do not touch (applies to all eight):** the `biased;` arm ordering in `App::run` (round 1's
invariant — **cancel, then input, then everything else** — is load-bearing and pinned by
[`src/tests/run_loop_input_priority.rs`](../../../crates/cyrup-tui/src/tests/run_loop_input_priority.rs));
the TUI-090 `live_floor` release logic; the `insert_before` exactly-once discipline in
`flush_committed`; the reader-thread escape hatch.

---

## 7. Definition of done (aggregate)

Expressed as code properties of the patched tree, not as test coverage. Each property is owned by
the task file named beside it; the bar is that **all eight** hold.

1. **No production allocation scales with total committed output.** `scrollback` exists only under
   `#[cfg(any(test, feature = "scrollback-accumulator"))]`; the default-feature build of
   `cyrup-tui` contains no reference to it (`cargo build -p cyrup-tui` compiles with the gate off).
   — [F1](TUI-092-F1-scrollback-accumulator.md)
2. **A frame with unchanged state is O(changed chrome).** One spinner tick with no new content
   performs zero markdown parses, zero syntect highlights, zero image rasterisations, zero wrap
   measurements — verifiable by reading `cached_render`'s key check. One delta → one
   materialisation, never three. — [F2](TUI-092-F2-transcript-render-cache.md) + [F7](TUI-092-F7-image-renderer-protocol-cache.md)
3. **One wakeup, one frame.** The `events`, `input` and `bash_next` arms each drain every
   immediately-ready message before their single `draw_synchronized()`, and the events arm moves
   (not clones) each payload. — [F3](TUI-092-F3-draw-coalescing.md) + [F8](TUI-092-F8-by-value-ingest.md)
4. **No per-event work walks history.** `context_usage` holds one manager lock and does one reverse
   branch scan with zero message clones. — [F4](TUI-092-F4-context-usage-reverse-scan.md)
5. **Commit frames keep the cell diff.** The `scrolling-regions` feature is compiled in
   (`cargo tree -p cyrup-tui -e features | grep scrolling-regions` resolves), so
   `insert_before_scrolling_regions` — not the `clear()` path — serves every flush. — [F5](TUI-092-F5-scrolling-regions.md)
6. **Every live collection has a stated bound.** `BashExecution::output_lines` ≤ 2000 lines + an
   omission counter; the §5.9 table remains true; nothing new unbounded is introduced. — [F6](TUI-092-F6-bash-output-ring.md) (+ §5.9)
7. **Round 1's invariants are intact**: arm order unchanged (cancel → input → rest), the escape
   hatch still fires from the reader thread, `mark_input_serviced()` still means "serviced". — all
   eight (pinned by the §6 "Do not touch" list)

The bar, restated from §5: **a session that runs for hours costs what a fresh one costs** — per
frame, per event, and in retained memory.

---

## 8. Open questions for the reporter

Answer these during round 2 — the first one still sets the severity of this row.

1. **In the locked-up state, do three deliberate `Ctrl+C` presses (about half a second apart) still
   exit and leave a usable shell?** Round 1's escape hatch lives in the input reader thread, which is
   independent of the render loop, so in principle it should still fire. If it does, this row is
   **high** — the app becomes unusable but the user is never trapped. If it does not, it is
   **critical** and the escape hatch has a second hole in it.
2. **Roughly how long, or how many turns, until phase 2 (spinner lag) is noticeable?** Minutes or
   hours; ten turns or a hundred. Any number bounds the growth rate.
3. **Does anything make it arrive sooner** — long replies, code-heavy answers, images, many tool
   calls, `/resume`ing a large session? Or is it just elapsed turns regardless of content?
4. **Does the process memory footprint climb visibly** over the session (`top` / Activity Monitor on
   the `cyrup` process)? *(F1 predicts yes, strongly, on long sessions.)*
5. **Does `/new` reset it**, or does a fresh session inside the same process stay slow? *(F1
   predicts it does NOT reset — the accumulator survives session swap; F4 predicts the per-event
   stall DOES reset, since the new branch is short.)*

---

## 9. Cross-references

* Round 1's landed work and QA sign-off — the three closed mechanisms in §2.
* `TUI-090` (post-turn whitespace) — **FIXED 2026-08-15**; its `live_floor` × `insert_before`
  machinery is on the per-frame path and is **preserved untouched** by this spec (§6, "Do not
  touch").
* `TUI-091` (reasoning blocks never render) — still open; re-check after this row lands, per the
  ledger note.
* Third-party sources consulted (vendored for citation):
  [`tmp/ratatui-core-0.1.2/src/terminal/inline.rs`](../../../tmp/ratatui-core-0.1.2/src/terminal/inline.rs)
  (both `insert_before` paths),
  [`tmp/ratatui-crossterm-0.1.2/src/lib.rs`](../../../tmp/ratatui-crossterm-0.1.2/src/lib.rs)
  (the `scroll_region_up/down` backend impls),
  [`tmp/ratatui-image-11.0.6/src/picker.rs`](../../../tmp/ratatui-image-11.0.6/src/picker.rs)
  (`Picker::new_protocol`, the per-frame encode F7 caches).