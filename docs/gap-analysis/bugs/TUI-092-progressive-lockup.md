# TUI-092 — The TUI degrades over the life of a session and ends in a total lockup

> **Status** — **ALL EIGHT FIXES LANDED — `F8` closed 2026-08-20.** Round 1 landed on
> 2026-08-15 and measurably improved the app; it did not eliminate the defect. Round 2's audit
> (§5) is **done** — every collection, cache, buffer, channel, task, subscription and terminal-side
> write path in `crates/cyrup-tui` was classified against the owner's bar — and `F1`–`F8` are all in
> the tree, each verified line by line in §6.
>
> **`F8` (by-value ingest) HAD NEVER BEEN WRITTEN** — `e6f298d` (*"…land TUI-092 F5-F8"*) deleted its
> task file while its diff touched only `crates/cyrup-tui/src/image.rs` (F7), a `cyrup-provider`
> proxy test and four deleted markdown files. **It is now written** (2026-08-20): the fold is
> `ingest_event_rendered_owned(&mut self, ev: AgentSessionEvent, …)`
> ([`app/events_fold.rs:20-22`](../../../crates/cyrup-tui/src/app/events_fold.rs#L20)), the run loop
> calls `self.ingest_session_event_owned(ev, &ctx.session).await;`
> ([`app/run_action.rs:285`](../../../crates/cyrup-tui/src/app/run_action.rs#L285)), and all four
> clones the row named are gone — `args` moves at
> [`:175-180`](../../../crates/cyrup-tui/src/app/events_fold.rs#L175), `partial_result` at
> [`:189`](../../../crates/cyrup-tui/src/app/events_fold.rs#L189), `result` at
> [`:195-201`](../../../crates/cyrup-tui/src/app/events_fold.rs#L195) and both queue vectors at
> [`:216`](../../../crates/cyrup-tui/src/app/events_fold.rs#L216). `grep -rn 'args.clone()\|partial_result.clone()\|result.clone()\|steering.clone()' crates/cyrup-tui/src/app/events_fold.rs`
> now returns nothing. **§7 property 3 is MET.** *(A deleted task file was never a landed fix — that
> lesson stands and is restated in §6.1.)*
>
> **RE-BASELINED 2026-08-19 against HEAD (`4fb5e40`).** Every anchor in §5.0/§5.9/§6 was re-read in
> the tree. `40821ed` split `crates/cyrup-tui/src/app.rs` into `crates/cyrup-tui/src/app/` (33
> modules), so **that file no longer exists**: every `app.rs:NNNN` citation and every
> `…/app.rs#LNNNN` hyperlink below has been re-pointed at the symbol's current module and line.
>
> **This file is the umbrella/index, and it is now a LANDED LEDGER, not a plan.** §6 records what
> each defect did, where it landed and the commit that landed it. **It no longer links to child task
> files, because none exist.** `F1`/`F2`/`F3` never had one — they landed before this file was
> written — and `F4`–`F8`'s were created by `7ff86f5`, then deleted by `425ef9f` (F4) and `e6f298d`
> (F5–F8). At HEAD `docs/gap-analysis/bugs/` holds exactly two files: this one and
> [`SUBA-072`](SUBA-072-scratch-dir-project-scope.md). The per-defect detail that lived in
> §5.1–§5.8 went with those children; what survives here is the shared context (§1–§4), the
> re-anchored audit trail (§5.0), the cleared list (§5.9), the landed ledger (§6), the aggregate
> definition of done (§7), the open questions (§8) and the cross-references (§9).
>
> **Kind** `cyrup-original` · **Severity** **high** — *no longer "pending the escape-hatch answer in
> §8": §8 Q1's `critical` branch is REFUTED IN CODE (see §8), so **high** is settled. Any area file
> still carrying `critical` for this row is stale against this file.* · **Effort** **M** *(remaining
> code scope: none — all eight fixes are in the tree)*

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

The audit is now complete, and the answer is **not one culprit — it is eight, and they compound**.
The right-hand column is the **status at HEAD**, re-verified 2026-08-19; the per-defect evidence is
in §6.

| # | Defect (as diagnosed) | Cost shape | Phase it drives | Landed? |
| --- | --- | --- | --- | --- |
| F1 | `AppState::scrollback` — every committed line cloned into a test-only accumulator, retained for the process lifetime, never cleared on session swap | memory ∝ **total session output** | 3→4 (swap pressure; the only structure that grows with *session age*) | **YES** — `f22efab` (the `cfg` gates) + `40821ed` (the feature declaration) |
| F2 | The active region is materialised **3× per frame** — full markdown parse + syntect highlight + image rasterisation of the whole streaming turn, plus two full `Vec<Line>` clones for wrap measurement | CPU/frame ∝ **active turn size** | 2→3 (the spinner canary: one frame crosses 80 ms) | **YES** — `f14a5db`, tests `c1d4a9a` |
| F3 | **No draw coalescing** — one full frame per session event, per bash chunk, per keystroke | frames/s ∝ event rate; with F2, a turn costs O(turn²) | 2→3 | **YES** — `f22efab`, tests `eed3e2d` |
| F4 | `refresh_context_usage` rebuilds the **entire branch message list** (with clones) on every `MessageEnd`/`AgentEnd`, awaited on the run-loop task | CPU/event ∝ **session history** | 3 (every turn's frame stalls a little more) | **YES** — `2086366` |
| F5 | ratatui `scrolling-regions` is **off**: every commit flush ends in `Terminal::clear()` → the next frame is a **full viewport repaint**, not a cell diff | bytes/frame spike per commit | 2 (commit cadence during a turn) | **YES** — `425ef9f` (no dedicated test) |
| F6 | `BashExecution::output_lines` accumulates **every** output line of a live `!`/`!!` run; the session-side sink forwards every chunk uncapped | memory ∝ run output | 2→3 during chatty runs | **YES** — `8f10804` (no dedicated test) |
| F7 | `ImageRenderer::render` re-encodes the image protocol (raster clone + resize + base64) **every frame** per attached image | CPU/frame ∝ attached image px | 2 while attachments sit | **YES** — `e6f298d` (no dedicated test) |
| F8 | The run loop's event ingest **clones** `args` / `partial_result` / `result` / queue vectors per event instead of moving them | CPU/event ∝ payload size | 3 | **YES** — 2026-08-20, by-value ingest end to end (no dedicated test; the structural guard in `run_loop_draw_coalescing.rs` pins the call site) |

The compounding is the lockup: F2 makes frames expensive; F3 multiplies expensive frames by event
rate; F5 makes every commit frame a full repaint; F4 makes the stall grow with every turn; F1 grows
memory until the allocator and the OS compressor join in; and once frame cost × event rate > 1 the
loop falls permanently behind — the de facto lockup of phase 4, reached with no error and no panic,
exactly as reported.

**That paragraph is the DIAGNOSIS, not the state of the tree.** Seven of the eight are fixed at HEAD
(§6), so the compounding above no longer describes a build anyone can run. What is *not* established
is whether seven is enough: **the defect has not been re-observed live since round 2 landed**, and
per this workspace's own rule a TUI claim is not settled by `TestBackend` — see §8.

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

The eight defects are ledgered in §6 — **one row each, no child files: none exist at HEAD** (see the
status block). §5.9 is the list of structures that were audited and **cleared** — do not re-audit
them.

---

### §5.0 — Audit anchors, RE-ANCHORED AT HEAD (read in the tree, not assumed)

Every finding was confirmed against the working tree before its fix was written, and every anchor was
then **re-read and re-pointed on 2026-08-19** — `40821ed` deleted `app.rs`, and `f14a5db` / `f22efab`
/ `2086366` / `e6f298d` moved or rewrote the very lines this table cites. Where a landed fix changed
a **signature** and not merely a line number, that is called out: a table that still names the
pre-fix shape misleads worse than one that is merely misnumbered.

| # | Anchor at HEAD | What it confirms |
| --- | --- | --- |
| F1 | field [`app/state.rs:137-138`](../../../crates/cyrup-tui/src/app/state.rs#L137) (the `#[cfg(any(test, feature = "scrollback-accumulator"))]` gate at `:137`, `pub scrollback: Vec<Line<'static>>` at `:138`); extend [`app/draw.rs:168-169`](../../../crates/cyrup-tui/src/app/draw.rs#L168); accessors [`app/shell.rs:273`](../../../crates/cyrup-tui/src/app/shell.rs#L273) (`scrollback_lines`) / [`:288`](../../../crates/cyrup-tui/src/app/shell.rs#L288) (`scrollback_text`), both behind the same gate; feature declared [`cyrup-tui/Cargo.toml:27`](../../../crates/cyrup-tui/Cargo.toml#L27) | `rebind_session` is [`app/session_bind.rs:4`](../../../crates/cyrup-tui/src/app/session_bind.rs#L4) — **not** `app/extension_ui.rs`, whose `reset_extension_ui` ([`:438`](../../../crates/cyrup-tui/src/app/extension_ui.rs#L438)) is merely its first statement. It resets transcript / selector / overlays / streaming / both queues / indicator / `live_floor` (`:16-31`) and **never touches `scrollback`**, which is why the accumulator had to leave the production build rather than be cleared on swap. **The original row's "the only two non-`src/tests/` in-crate accessor call sites" no longer describes anything:** the split moved both out of `app.rs` and they are now literally in `src/tests/` ([`tests/bash_live_run.rs:73`](../../../crates/cyrup-tui/src/tests/bash_live_run.rs#L73)/[`:112`](../../../crates/cyrup-tui/src/tests/bash_live_run.rs#L112)). The external consumer is [`cyrup-it/tests/bin/wasm_renderer_screen.rs:119`](../../../crates/cyrup-it/tests/bin/wasm_renderer_screen.rs#L119)/[`:144`](../../../crates/cyrup-it/tests/bin/wasm_renderer_screen.rs#L144), and its crate now pins `cyrup-tui = { workspace = true, features = ["scrollback-accumulator"] }` at [`cyrup-it/Cargo.toml:99`](../../../crates/cyrup-it/Cargo.toml#L99) — the feature did go there. |
| F2 | `render_generation` [`transcript.rs:352`](../../../crates/cyrup-tui/src/transcript.rs#L352); `RenderCache` [`:366`](../../../crates/cyrup-tui/src/transcript.rs#L366); `bump_render_generation` [`:1209`](../../../crates/cyrup-tui/src/transcript.rs#L1209); `bump_render_tick` [`:1220`](../../../crates/cyrup-tui/src/transcript.rs#L1220); `cached_render` [`:1228-1249`](../../../crates/cyrup-tui/src/transcript.rs#L1228); `content_height(&mut self)` [`:1254`](../../../crates/cyrup-tui/src/transcript.rs#L1254) → `lines(&self)` [`:1258`](../../../crates/cyrup-tui/src/transcript.rs#L1258); `region_constraints(state: &mut AppState)` [`app/layout.rs:48`](../../../crates/cyrup-tui/src/app/layout.rs#L48); its `content_height` call [`:165`](../../../crates/cyrup-tui/src/app/layout.rs#L165); `live_region_height(state: &mut AppState)` [`:185`](../../../crates/cyrup-tui/src/app/layout.rs#L185); `render` re-calls `region_constraints` [`app/render.rs:7`](../../../crates/cyrup-tui/src/app/render.rs#L7); `TranscriptView::render` [`transcript.rs:3376`](../../../crates/cyrup-tui/src/transcript.rs#L3376) now calls `cached_render` at [`:3379`](../../../crates/cyrup-tui/src/transcript.rs#L3379) | **The `&mut` propagation landed and is borrow-clean — and the SIGNATURE CHANGED.** `content_height` is `&mut self` at HEAD; any doc still writing `content_height(&self)` is describing the pre-`f14a5db` tree. `TranscriptView::render` was already `&mut self`, and `render` owns `state: &mut AppState`; `draw` calls `live_region_height` at [`app/draw.rs:46`](../../../crates/cyrup-tui/src/app/draw.rs#L46) **before** destructuring `self` into `terminal`/`state` at [`:89`](../../../crates/cyrup-tui/src/app/draw.rs#L89) and handing `state` to `render` via `terminal.draw(\|frame\| render(frame, state))` at [`:90`](../../../crates/cyrup-tui/src/app/draw.rs#L90); `App::new` ([`app/shell.rs:8`](../../../crates/cyrup-tui/src/app/shell.rs#L8)) calls it on an owned local `state` at [`:11`](../../../crates/cyrup-tui/src/app/shell.rs#L11). All four call sites supply `&mut` with no borrow conflict. The key is `(render_generation, width, theme.generation)`, and `RenderCache` is a value rather than an `Option` because the workspace no-panic lints forbid the `expect` an `Option` forces on the re-borrow. Pinned by [`src/tests/render_cache_tick.rs`](../../../crates/cyrup-tui/src/tests/render_cache_tick.rs) and `transcript.rs:5417` (`render_paints_the_cached_lines_not_a_recompute`). |
| F3 | `events` drain [`app/run_action.rs:267-270`](../../../crates/cyrup-tui/src/app/run_action.rs#L267) in `App::on_session_event` ([`:242`](../../../crates/cyrup-tui/src/app/run_action.rs#L242)), single draw [`:298`](../../../crates/cyrup-tui/src/app/run_action.rs#L298); `input` drain [`:217-219`](../../../crates/cyrup-tui/src/app/run_action.rs#L217) in `App::on_input_event` ([`:193`](../../../crates/cyrup-tui/src/app/run_action.rs#L193)), single draw [`:230`](../../../crates/cyrup-tui/src/app/run_action.rs#L230); `bash_next` drain [`app/run_arms.rs:353-356`](../../../crates/cyrup-tui/src/app/run_arms.rs#L353) in `App::on_bash_msg` ([`:346`](../../../crates/cyrup-tui/src/app/run_arms.rs#L346)), single draw [`:387`](../../../crates/cyrup-tui/src/app/run_arms.rs#L387) | `now_or_never` is `futures::FutureExt::now_or_never`; `futures = { version = "0.3" }` is the workspace dep at [`Cargo.toml:125`](../../../Cargo.toml#L125) (**not `:122`** — the root manifest moved) and already a direct dep of `cyrup-tui` ([`cyrup-tui/Cargo.toml:65`](../../../crates/cyrup-tui/Cargo.toml#L65)) — **no new dependency**. The bash arm uses `try_recv`, not `now_or_never`, because `bash_rx` is the concrete `UnboundedReceiver` in scope, so its drain constructs no future. The reader thread's channel is `unbounded_channel::<InputEvent>()` at [`app/input_reader.rs:313`](../../../crates/cyrup-tui/src/app/input_reader.rs#L313) (a `std::thread` that cannot `.await`) and stays unbounded by design; ONE `ArmGuard::enter("events")` ([`app/run_action.rs:255`](../../../crates/cyrup-tui/src/app/run_action.rs#L255)) brackets the whole drain, so the wedge detector sees one span rather than N. Pinned by [`src/tests/run_loop_draw_coalescing.rs`](../../../crates/cyrup-tui/src/tests/run_loop_draw_coalescing.rs). |
| F4 | `context_usage` [`session.rs:4146-4186`](../../../crates/cyrup-session-svc/src/session.rs#L4146): the model read first ([`:4158`](../../../crates/cyrup-session-svc/src/session.rs#L4158)), then ONE `self.manager.lock().await` ([`:4160`](../../../crates/cyrup-session-svc/src/session.rs#L4160)), then ONE `guard.branch_path(None).into_iter().rev()` ([`:4173-4176`](../../../crates/cyrup-session-svc/src/session.rs#L4173)), zero message clones, into `ContextUsage::from_last_assistant` ([`:4185`](../../../crates/cyrup-session-svc/src/session.rs#L4185)) | The old path — `messages()` ([`session.rs:3942`](../../../crates/cyrup-session-svc/src/session.rs#L3942)) → `build_context` ([`manager.rs:737`](../../../crates/cyrup-session/src/manager.rs#L737)) → `build_context_messages` (*defined* in [`cyrup-session/src/context.rs:151`](../../../crates/cyrup-session/src/context.rs#L151), called from [`manager.rs:766`](../../../crates/cyrup-session/src/manager.rs#L766), **never** in `manager.rs` itself) — is off the per-event path entirely. `branch_path` is [`manager.rs:627`](../../../crates/cyrup-session/src/manager.rs#L627); `from_last_assistant` is [`state.rs:285`](../../../crates/cyrup-session-svc/src/state.rs#L285); `has_post_compaction_usage` ([`session.rs:4093`](../../../crates/cyrup-session-svc/src/session.rs#L4093)) already walked `entries()` clone-free and is the shape the rewrite copied — including `filter_map(..).find(..)` rather than `find_map`, so a `StopReason::Deferred` tail does not stop the scan. `Message` was already in scope at [`session.rs:13`](../../../crates/cyrup-session-svc/src/session.rs#L13); no new top-level `use` was needed. |
| F5 | feature [`cyrup-tui/Cargo.toml:33`](../../../crates/cyrup-tui/Cargo.toml#L33) (`scrolling-regions = ["ratatui/scrolling-regions"]`), **ON by default** at [`:19`](../../../crates/cyrup-tui/Cargo.toml#L19); the two backend delegations the flip compiles are [`app/backend.rs:192-200`](../../../crates/cyrup-tui/src/app/backend.rs#L192) | ratatui dispatches on the feature at [`tmp/ratatui-core-0.1.2/src/terminal/inline.rs:113`](../../../tmp/ratatui-core-0.1.2/src/terminal/inline.rs#L113); the no-regions `self.clear()?` is [`inline.rs:212`](../../../tmp/ratatui-core-0.1.2/src/terminal/inline.rs#L212) (tmux-workaround comment `:209-211`), and `insert_before_scrolling_regions` starts at [`:228`](../../../tmp/ratatui-core-0.1.2/src/terminal/inline.rs#L228); crossterm's `scroll_region_up`/`down` impls are [`tmp/ratatui-crossterm-0.1.2/src/lib.rs:362`](../../../tmp/ratatui-crossterm-0.1.2/src/lib.rs#L362)–`383`. The flip is ON by default precisely because it retires ratatui's own documented tmux clear+scroll garbage hazard rather than introducing one. |
| F6 | `output_lines: VecDeque<String>` [`bash.rs:57`](../../../crates/cyrup-tui/src/bash.rs#L57); `omitted_lines: usize` [`:60`](../../../crates/cyrup-tui/src/bash.rs#L60); `append_output` [`:137`](../../../crates/cyrup-tui/src/bash.rs#L137) with the front eviction at [`:152-155`](../../../crates/cyrup-tui/src/bash.rs#L152); `MAX_OUTPUT_LINES = 2000` [`:29`](../../../crates/cyrup-tui/src/bash.rs#L29); the one dim omission row at [`:305-313`](../../../crates/cyrup-tui/src/bash.rs#L305) | 2000 is deliberately the SAME bound [`cyrup-tools/src/truncate.rs:11`](../../../crates/cyrup-tools/src/truncate.rs#L11)'s `DEFAULT_MAX_LINES` applies to a finished result, so the live block and the finished one agree. The session-side sink still forwards every chunk — its own bound is `ROLLING_MAX_BYTES` (`DEFAULT_MAX_BYTES * 2` = 100 KB, [`cyrup-session-svc/src/bash.rs:294`](../../../crates/cyrup-session-svc/src/bash.rs#L294), evicted at [`:330`](../../../crates/cyrup-session-svc/src/bash.rs#L330)) plus the temp-file spill, and it applies to the result preview, not to the live rows. |
| F7 | `ImageRenderer` [`image.rs:50`](../../../crates/cyrup-tui/src/image.rs#L50) now holds `protocol_cache: Mutex<HashMap<ImageCacheKey, Protocol>>` [`:58`](../../../crates/cyrup-tui/src/image.rs#L58); key type `ImageCacheKey` [`:33`](../../../crates/cyrup-tui/src/image.rs#L33); `render(&self, …)` [`:175`](../../../crates/cyrup-tui/src/image.rs#L175); the memoised `new_protocol(block.image.clone(), …)` [`:211`](../../../crates/cyrup-tui/src/image.rs#L211), reached only on a miss ([`:210`](../../../crates/cyrup-tui/src/image.rs#L210)); `render_images` [`app/render.rs:227`](../../../crates/cyrup-tui/src/app/render.rs#L227) | `Picker::new_protocol` returns an owned, reusable `Protocol` ([`tmp/ratatui-image-11.0.6/src/picker.rs:256`](../../../tmp/ratatui-image-11.0.6/src/picker.rs#L256)) — caching it is the library's own `StatefulImage` pattern. `render` is still `&self`, so the cache is interior-mutable, and a poisoned lock degrades to `into_inner()` rather than propagating ([`:206-209`](../../../crates/cyrup-tui/src/image.rs#L206)) under the no-panic policy. **The eviction half of the original row is no longer true as written:** `pending_images.clear()` now lives in `App::clear_images` ([`app/shell.rs:361-363`](../../../crates/cyrup-tui/src/app/shell.rs#L361)) and has **no production caller at all** — see §5.9, which restates that bound. |
| F8 | **LANDED 2026-08-20.** The fold is by value — `ingest_event_rendered_owned(&mut self, ev: AgentSessionEvent, …)` [`app/events_fold.rs:20-22`](../../../crates/cyrup-tui/src/app/events_fold.rs#L20) — and the arms MOVE the payloads: `args` [`:175-180`](../../../crates/cyrup-tui/src/app/events_fold.rs#L175), `partial_result` [`:189`](../../../crates/cyrup-tui/src/app/events_fold.rs#L189), `result` [`:195-201`](../../../crates/cyrup-tui/src/app/events_fold.rs#L195), `(steering, follow_up)` [`:216`](../../../crates/cyrup-tui/src/app/events_fold.rs#L216). The run loop calls the owned ingest at [`app/run_action.rs:285`](../../../crates/cyrup-tui/src/app/run_action.rs#L285) → `ingest_session_event_owned` [`app/events.rs:90`](../../../crates/cyrup-tui/src/app/events.rs#L90) → `ingest_event_with_extensions_owned` [`:49`](../../../crates/cyrup-tui/src/app/events.rs#L49) → the fold. The by-reference entry points survive as thin `ev.clone()` wrappers for the ~253 in-crate test call sites and `cyrup-it`'s renderer-screen bin ([`app/events.rs:17`](../../../crates/cyrup-tui/src/app/events.rs#L17), [`:35`](../../../crates/cyrup-tui/src/app/events.rs#L35), [`:80`](../../../crates/cyrup-tui/src/app/events.rs#L80)), so no production path pays a clone. Two hoists the move forces, both order-preserving: `context_usage_may_have_moved(&ev)` is read before the fold consumes `ev` ([`app/events.rs:100`](../../../crates/cyrup-tui/src/app/events.rs#L100)) and the `edit` diff is computed before the row is pushed ([`app/events_fold.rs:160-165`](../../../crates/cyrup-tui/src/app/events_fold.rs#L160)) — the two transcript mutations still run push-then-`set_edit_preview`. **What it replaced:** the borrow `ev: &AgentSessionEvent` [`app/events_fold.rs:9`](../../../crates/cyrup-tui/src/app/events_fold.rs#L9) (fn `ingest_event_rendered` at [`:7`](../../../crates/cyrup-tui/src/app/events_fold.rs#L7)); the four surviving clones [`:141`](../../../crates/cyrup-tui/src/app/events_fold.rs#L141) (`args.clone()`), [`:163`](../../../crates/cyrup-tui/src/app/events_fold.rs#L163) (`partial_result.clone()`), [`:173`](../../../crates/cyrup-tui/src/app/events_fold.rs#L173) (`result.clone()`), [`:190`](../../../crates/cyrup-tui/src/app/events_fold.rs#L190) (`(steering.clone(), follow_up.clone())`); the un-swapped call [`app/run_action.rs:282`](../../../crates/cyrup-tui/src/app/run_action.rs#L282) under the still-future-tense comment at [`:281`](../../../crates/cyrup-tui/src/app/run_action.rs#L281) | The receiving transcript APIs already consume by value — `push_tool_start_rendered(…, args: Value, …)` [`transcript.rs:783`](../../../crates/cyrup-tui/src/transcript.rs#L783), `push_tool_update(…, partial: Option<Value>)` [`:813`](../../../crates/cyrup-tui/src/transcript.rs#L813), `push_tool_end_rendered(…, result: Option<Value>, …)` [`:882`](../../../crates/cyrup-tui/src/transcript.rs#L882) — so the clones exist **only** because the ingest path borrows `&ev`, exactly as diagnosed. **Nothing about the fix was invalidated; it was simply never written.** `f22efab` had pre-staged the call site: `info_changed`/`settled` are computed BEFORE the ingest call ([`app/run_action.rs:274-275`](../../../crates/cyrup-tui/src/app/run_action.rs#L274)), which is why the swap was one line. |

**Hyperlink convention:** every path is relative to this directory (`docs/gap-analysis/bugs/`), so
`../../../crates/…` resolves into the workspace root and `../../../tmp/…` into the vendored
third-party sources used for the F5/F7 citations. **Two caveats added 2026-08-19:** (a) there are no
longer any child task files in this directory to share the convention with (see the status block);
and (b) `06eff7a` added `tmp/` to `.gitignore`, so the four `../../../tmp/…` targets resolve only in
a checkout that has the vendored copies — they are cited as evidence, not as tracked repo content.

---

### §5.9 — Audited and CLEARED (do not re-audit)

These were traced to their bounds and are **not** defects under the bar. **Every anchor re-read
2026-08-19.** One row's *bound* — not merely its line number — had drifted and is restated in full
(`pending_images`); one more (the tickers) gained the clause F2's `bump_render_tick` added.

| Structure | Bound |
| --- | --- |
| Editor undo stack | capped at 500 — one guard per push path: `push_undo_for` [`editor.rs:923`](../../../crates/cyrup-tui/src/editor.rs#L923) and `push_undo_for_type` [`:937`](../../../crates/cyrup-tui/src/editor.rs#L937), both `remove(0)` on overflow |
| Editor prompt history | capped at 100 — the constant is [`HISTORY_CAP`](../../../crates/cyrup-tui/src/editor.rs#L41) (`editor.rs:41`), enforced at [`:1517`](../../../crates/cyrup-tui/src/editor.rs#L1517) |
| `TranscriptView::pending` | drained every frame by [`drain_committed`](../../../crates/cyrup-tui/src/transcript.rs#L596) (`transcript.rs:596`, `mem::take`) |
| `active_tools` / `streaming` / `thinking` / `bash` | committed at turn end ([`commit_tools`](../../../crates/cyrup-tui/src/transcript.rs#L955), `transcript.rs:955`); finished tools flushed mid-turn ([`commit_finished_leading_tools`](../../../crates/cyrup-tui/src/transcript.rs#L984), `:984`) |
| Session event `Fanout` channels | bounded (`CHANNEL_CAPACITY = 1024`, [`subscriber.rs:23`](../../../crates/cyrup-session-svc/src/subscriber.rs#L23)), closed senders pruned per emit ([`:64-76`](../../../crates/cyrup-session-svc/src/subscriber.rs#L64), `retain` at `:75`) |
| Run-scoped subscriptions | cleared on settle (`Fanout::end_run`, [`subscriber.rs:82`](../../../crates/cyrup-session-svc/src/subscriber.rs#L82)); persistent ones dropped on swap; `invalidate` ([`:89`](../../../crates/cyrup-session-svc/src/subscriber.rs#L89)) clears both — and **that same `invalidate` is what `TUI-094` turned out to hinge on** (§9) |
| Tool-result image rasters | decoded **once** at `ToolExecutionEnd`, downscaled to [`MAX_RASTER_PX` = 1024](../../../crates/cyrup-tui/src/transcript.rs#L1374) (`transcript.rs:1374`, applied at [`:1608`](../../../crates/cyrup-tui/src/transcript.rs#L1608)) |
| Bash result preview (session side) | rolling 100 KB cap (`ROLLING_MAX_BYTES = DEFAULT_MAX_BYTES * 2`, [`cyrup-session-svc/src/bash.rs:294`](../../../crates/cyrup-session-svc/src/bash.rs#L294), evicted at [`:330`](../../../crates/cyrup-session-svc/src/bash.rs#L330)) + temp-file spill |
| `pending_images` (attachments) | **BOUND RESTATED 2026-08-19 — it is not "cleared on submit".** `App::clear_images` ([`app/shell.rs:361-363`](../../../crates/cyrup-tui/src/app/shell.rs#L361)) is the only writer that empties it and has **no production caller**: `grep -rn 'clear_images' crates/` reaches only that definition, one doc comment ([`image.rs:56`](../../../crates/cyrup-tui/src/image.rs#L56)) and [`src/tests/image.rs:116`](../../../crates/cyrup-tui/src/tests/image.rs#L116). The `Submit` arm ([`app/run_action.rs:83`](../../../crates/cyrup-tui/src/app/run_action.rs#L83)) does not touch it. It stays cleared in practice only because **its two producers are equally uncalled outside tests** — `attach_image` ([`app/shell.rs:294`](../../../crates/cyrup-tui/src/app/shell.rs#L294)) and `attach_image_path` ([`:300`](../../../crates/cyrup-tui/src/app/shell.rs#L300)) have no caller anywhere in `crates/` (both are `pub` on `App`, so an embedder outside the workspace could still fill it), since clipboard paste now inserts the temp-file **path as text** instead ([`insert_clipboard_image_path`](../../../crates/cyrup-tui/src/app/shell.rs#L318), `app/shell.rs:318`, pi's literal mechanism). **Still cleared under the bar — the collection is provably empty in production — but for a different reason than this row claimed, and a future `@`-mention image source would reintroduce the unbounded case with no clear on its path.** |
| `EscapeReassembler` / `StrayReplyFilter` held buffers | flushed on every input-idle tick ([`app/input_reader.rs:356-363`](../../../crates/cyrup-tui/src/app/input_reader.rs#L356) — the `Ok(false)` arm flushes the reassembler, drains it through the filter, then flushes the filter, so both release on the SAME tick) |
| `extension_statuses` | `BTreeMap` keyed by extension id ([`status.rs:132`](../../../crates/cyrup-tui/src/status.rs#L132)); blank values remove entries ([`:258-260`](../../../crates/cyrup-tui/src/status.rs#L258)) |
| `session_queue` / `pending_messages` / `compaction_queue` | replaced per `queue_update`; cleared on session swap ([`app/session_bind.rs:26-28`](../../../crates/cyrup-tui/src/app/session_bind.rs#L26)) |
| ratatui `Terminal` buffers | sized to the viewport; rebuilt on resize; `insert_before` retains nothing (verified in [`ratatui-core inline.rs`](../../../tmp/ratatui-core-0.1.2/src/terminal/inline.rs#L130)) |
| Syntect `SyntaxSet` | process-wide `OnceLock`, built once ([`markdown.rs:1742`](../../../crates/cyrup-tui/src/markdown.rs#L1742)) |
| Spinner/dialog/progress/git/elapsed tickers | `MissedTickBehavior::Skip`, `if`-gated, idempotent — and, since F2, the spinner and elapsed ticks call `bump_render_tick` so a time-derived frame still re-materialises without making quiet frames pay |
| Per-submission / per-shortcut spawned tasks | end with their op; channels dropped with them |
| `App::live_floor` ([`app/mod.rs:206`](../../../crates/cyrup-tui/src/app/mod.rs#L206)) / `viewport_height` | scalars; the TUI-090 floor release ([`app/draw.rs:66-71`](../../../crates/cyrup-tui/src/app/draw.rs#L66)) is preserved untouched |

The one structural fact that makes the cleared list possible: committed history leaves the process
entirely — it is written to the terminal's native scrollback exactly once
([`flush_committed`](../../../crates/cyrup-tui/src/app/draw.rs#L128), `app/draw.rs:128`) and never re-rendered. What the
emulator itself retains is terminal-side state, outside this process's reach; the app's obligation
under the bar is to write each line once and keep nothing — which, after F1, it does.

---

## 6. Landed ledger — what each defect did, and where it is at HEAD

**This section was a plan; it is now a record.** It listed eight child task files in landing order;
none of those files exists at HEAD (see the status block), so the links are gone and each row now
carries its landing commit and the anchors a reader can verify. Rows are in the order the work
actually landed, which is **not** the order the plan proposed.

| Order landed | Defect | What landed | Commit(s) | Verify at HEAD |
| --- | --- | --- | --- | --- |
| 1 | **F2** — `TranscriptView` render cache | `render_generation` + `RenderCache` + `cached_render`, keyed `(generation, width, theme.generation)`; 33 `&mut self` mutators bump it; `content_height` became `&mut self` and the `&mut` threaded through `region_constraints` / `live_region_height` / `render`. The big one | `f14a5db`; tests `c1d4a9a` | [`transcript.rs:352`](../../../crates/cyrup-tui/src/transcript.rs#L352), [`:366`](../../../crates/cyrup-tui/src/transcript.rs#L366), [`:1228-1249`](../../../crates/cyrup-tui/src/transcript.rs#L1228), [`:1254`](../../../crates/cyrup-tui/src/transcript.rs#L1254); [`app/layout.rs:48`](../../../crates/cyrup-tui/src/app/layout.rs#L48)/[`:185`](../../../crates/cyrup-tui/src/app/layout.rs#L185); [`src/tests/render_cache_tick.rs`](../../../crates/cyrup-tui/src/tests/render_cache_tick.rs) |
| 2 | **F3** — drain-then-draw | the `events`, `input` and `bash_next` arms each drain every immediately-ready message, then draw **once**; `'run` loop label so a drained `Quit` still exits mid-drain; one `ArmGuard` per drained arm | `f22efab`; tests `eed3e2d` | [`app/run_action.rs:218`](../../../crates/cyrup-tui/src/app/run_action.rs#L218), [`:267-270`](../../../crates/cyrup-tui/src/app/run_action.rs#L267); [`app/run_arms.rs:353`](../../../crates/cyrup-tui/src/app/run_arms.rs#L353); [`src/tests/run_loop_draw_coalescing.rs`](../../../crates/cyrup-tui/src/tests/run_loop_draw_coalescing.rs) |
| 2 | **F1** — `scrollback-accumulator` | the accumulator left the production build at compile time; `#[cfg(any(test, feature = "scrollback-accumulator"))]` on the field, the `extend` and both accessors. **Landed in two halves:** the `src/` gates rode in with F3; the `Cargo.toml` feature declaration and the two out-of-crate consumers came with the `app.rs` split | `f22efab` (gates) + `40821ed` (feature + consumers) | [`app/state.rs:137-138`](../../../crates/cyrup-tui/src/app/state.rs#L137); [`app/draw.rs:168-169`](../../../crates/cyrup-tui/src/app/draw.rs#L168); [`cyrup-tui/Cargo.toml:27`](../../../crates/cyrup-tui/Cargo.toml#L27); [`cyrup-it/Cargo.toml:99`](../../../crates/cyrup-it/Cargo.toml#L99) |
| 3 | **F4** — `context_usage` reverse scan | one manager lock, one reverse `branch_path` walk, zero message clones, in `cyrup-session-svc`; the model read is taken first so no lock nesting arises | `2086366` | [`cyrup-session-svc/src/session.rs:4146-4186`](../../../crates/cyrup-session-svc/src/session.rs#L4146) |
| 4 | **F5** — `scrolling-regions` | the one-line feature flip, **on by default**, so `insert_before_scrolling_regions` — not the `clear()` path — serves every flush; it also compiles `InlineBackend`'s two `scroll_region_*` delegations | `425ef9f` | [`cyrup-tui/Cargo.toml:19`](../../../crates/cyrup-tui/Cargo.toml#L19)+[`:33`](../../../crates/cyrup-tui/Cargo.toml#L33); [`app/backend.rs:192-200`](../../../crates/cyrup-tui/src/app/backend.rs#L192) |
| 5 | **F6** — bash output ring | `VecDeque` + `MAX_OUTPUT_LINES = 2000` front eviction + `omitted_lines` counter + one dim `… (N earlier lines omitted) …` row | `8f10804` | [`bash.rs:29`](../../../crates/cyrup-tui/src/bash.rs#L29), [`:57`](../../../crates/cyrup-tui/src/bash.rs#L57), [`:152-155`](../../../crates/cyrup-tui/src/bash.rs#L152), [`:305-313`](../../../crates/cyrup-tui/src/bash.rs#L305) |
| 6 | **F7** — `ImageRenderer` protocol cache | the built `Protocol` memoised in a `Mutex<HashMap<ImageCacheKey, Protocol>>` keyed on (image identity, target size); a poisoned lock degrades to `into_inner()` rather than propagating | `e6f298d` | [`image.rs:33`](../../../crates/cyrup-tui/src/image.rs#L33), [`:50`](../../../crates/cyrup-tui/src/image.rs#L50), [`:58`](../../../crates/cyrup-tui/src/image.rs#L58), [`:201-224`](../../../crates/cyrup-tui/src/image.rs#L201) |
| 7 | **F8** — by-value ingest | the fold takes the event by value (`ingest_event_rendered_owned`) and its arms MOVE `args` / `partial_result` / `result` / `steering` / `follow_up` into the transcript and `session_queue`; the run loop's drained events arm calls `ingest_session_event_owned(ev, …)`; the by-reference `ingest_event` / `ingest_event_with_extensions` / `ingest_session_event` remain as thin `ev.clone()` wrappers so the ~253 test call sites (and `cyrup-it`'s bin) compile untouched and no production path clones | 2026-08-20 *(uncommitted in the working tree when this row was written — verify by the anchors, not by a subject line)* | `grep -rn ingest_session_event_owned crates/` now hits the declaration [`app/events.rs:90`](../../../crates/cyrup-tui/src/app/events.rs#L90) and the call [`app/run_action.rs:285`](../../../crates/cyrup-tui/src/app/run_action.rs#L285); the fold [`app/events_fold.rs:20-22`](../../../crates/cyrup-tui/src/app/events_fold.rs#L20); the four ex-clone sites [`:175-180`](../../../crates/cyrup-tui/src/app/events_fold.rs#L175)/[`:189`](../../../crates/cyrup-tui/src/app/events_fold.rs#L189)/[`:195-201`](../../../crates/cyrup-tui/src/app/events_fold.rs#L195)/[`:216`](../../../crates/cyrup-tui/src/app/events_fold.rs#L216); the structural guard [`src/tests/run_loop_draw_coalescing.rs:113`](../../../crates/cyrup-tui/src/tests/run_loop_draw_coalescing.rs#L113) |

**F5, F6 and F7 landed with no dedicated test** — `grep -rl 'TUI-092' crates/cyrup-tui/src/tests/`
returns only `escalation.rs` (round 1's escape hatch), `render_cache_tick.rs` (F2),
`run_loop_draw_coalescing.rs` (F3) and `run_loop_input_priority.rs` (round 1's arm order). The three
untested fixes are each a small, structurally-verifiable change (a feature flag, a `VecDeque` cap, a
`HashMap` memo), which is why they shipped that way; recording it here so a later pass does not read
the silence as coverage.

**DO NOT TRUST A COMMIT SUBJECT AS LANDING EVIDENCE.** F8 is the standing counter-example: the
subject said it landed and the task file was deleted to prove it, while the only occurrence of
`ingest_session_event_owned` in the whole workspace was the comment promising to write it.
Before closing any row here, grep for the *artifact the fix was supposed to create* — for F8 that
grep now returns a declaration ([`app/events.rs:90`](../../../crates/cyrup-tui/src/app/events.rs#L90))
and a call ([`app/run_action.rs:285`](../../../crates/cyrup-tui/src/app/run_action.rs#L285)), which is
what closing a row is allowed to mean.

**Do not touch (applies to every remaining edit on this row, `F8` included):**

* **The `biased;` arm ordering in `App::run` — AMENDED, and deliberately so.** *(The code changed
  2026-08-18 in `879eb4e`; this instruction is corrected 2026-08-19, having forbidden that change.)*
  Round 1's invariant was **cancel, then input, then everything else**, pinned by
  [`src/tests/run_loop_input_priority.rs`](../../../crates/cyrup-tui/src/tests/run_loop_input_priority.rs). `879eb4e` (filed
  retroactively as `TUI-094`, §9) inserted the **session-swap arm between input and the tickers**:
  the rule at HEAD is **cancel → input → swap-rebind → everything else**
  ([`app/run.rs:264`](../../../crates/cyrup-tui/src/app/run.rs#L264) `biased;`, [`:265`](../../../crates/cyrup-tui/src/app/run.rs#L265) cancel,
  [`:278`](../../../crates/cyrup-tui/src/app/run.rs#L278) input, [`:293`](../../../crates/cyrup-tui/src/app/run.rs#L293) swap,
  [`:344`](../../../crates/cyrup-tui/src/app/run.rs#L344) events), pinned additionally by
  [`src/tests/run_loop_swap_arm_reachable.rs`](../../../crates/cyrup-tui/src/tests/run_loop_swap_arm_reachable.rs). The swap
  arm **must** outrank every arm that can be permanently ready, which a closed `events` stream is —
  `Fanout::invalidate` drops every sender on a swap. **Stated explicitly so the next reader does not
  revert a fix for a 100 %-CPU hang on this section's authority.** The round-1 half of the invariant
  (cancel and input on top) is untouched and still binding.
* The TUI-090 `live_floor` release logic ([`app/draw.rs:66-71`](../../../crates/cyrup-tui/src/app/draw.rs#L66)).
* The `insert_before` exactly-once discipline in `flush_committed`
  ([`app/draw.rs:128`](../../../crates/cyrup-tui/src/app/draw.rs#L128)).
* The reader-thread escape hatch ([`app/input_reader.rs:27-204`](../../../crates/cyrup-tui/src/app/input_reader.rs#L27), hard
  exit at [`:193`](../../../crates/cyrup-tui/src/app/input_reader.rs#L193)).

---

## 7. Definition of done (aggregate) — SCORED AT HEAD, 2026-08-19; property 3 RE-SCORED 2026-08-20

Expressed as code properties of the patched tree, not as test coverage. The bar is that **all seven**
hold, and **all seven now do** — property 3, the shape a missing F8 produced, closed when the
by-value ingest landed.

1. **MET — no production allocation scales with total committed output.** `scrollback` exists only
   under `#[cfg(any(test, feature = "scrollback-accumulator"))]`
   ([`app/state.rs:137`](../../../crates/cyrup-tui/src/app/state.rs#L137), [`app/draw.rs:168`](../../../crates/cyrup-tui/src/app/draw.rs#L168),
   [`app/shell.rs:272`](../../../crates/cyrup-tui/src/app/shell.rs#L272)/[`:287`](../../../crates/cyrup-tui/src/app/shell.rs#L287)), and the feature is
   **not** in `default` ([`cyrup-tui/Cargo.toml:19`](../../../crates/cyrup-tui/Cargo.toml#L19) is
   `["wasm-host", "scrolling-regions"]`). — F1
2. **MET — a frame with unchanged state is O(changed chrome).** `cached_render`'s key check
   ([`transcript.rs:1229-1231`](../../../crates/cyrup-tui/src/transcript.rs#L1229)) is the whole proof: an unchanged
   `(render_generation, width, theme.generation)` returns the cache without a parse, a highlight, a
   rasterisation or a wrap measurement, and `TranscriptView::render` reads the SAME cache
   ([`:3379`](../../../crates/cyrup-tui/src/transcript.rs#L3379)) rather than re-calling `lines()`, so one delta is one
   materialisation, never three. Timer-driven repaints stay correct via `bump_render_tick`
   ([`:1220`](../../../crates/cyrup-tui/src/transcript.rs#L1220)), gated on `bash_running()` / `has_running_elapsed_tool()` so
   content-quiet frames stay free. F7 supplies the image half. — F2 + F7
3. **MET — one wakeup, one frame, and the events arm MOVES each payload.** The drain landed: all
   three arms drain before a single `draw_synchronized()`
   ([`app/run_action.rs:218`](../../../crates/cyrup-tui/src/app/run_action.rs#L218)/[`:230`](../../../crates/cyrup-tui/src/app/run_action.rs#L230),
   [`:267-270`](../../../crates/cyrup-tui/src/app/run_action.rs#L267)/[`:301`](../../../crates/cyrup-tui/src/app/run_action.rs#L301),
   [`app/run_arms.rs:353`](../../../crates/cyrup-tui/src/app/run_arms.rs#L353)/[`:387`](../../../crates/cyrup-tui/src/app/run_arms.rs#L387)). So
   did the move: [`app/run_action.rs:285`](../../../crates/cyrup-tui/src/app/run_action.rs#L285) passes the dequeued
   `ev` **by value** into `ingest_session_event_owned`, and the fold
   ([`app/events_fold.rs:20`](../../../crates/cyrup-tui/src/app/events_fold.rs#L20)) moves `args` /
   `partial_result` / `result` / the queue vectors into the transcript rather than cloning them per
   event. — F3 + F8
4. **MET — no per-event work walks history.** `context_usage` holds one manager lock and does one
   reverse branch scan with zero message clones
   ([`cyrup-session-svc/src/session.rs:4160`](../../../crates/cyrup-session-svc/src/session.rs#L4160),
   [`:4173-4176`](../../../crates/cyrup-session-svc/src/session.rs#L4173)). — F4
5. **MET — commit frames keep the cell diff.** `scrolling-regions` is in `default`
   ([`cyrup-tui/Cargo.toml:19`](../../../crates/cyrup-tui/Cargo.toml#L19)) and forwards to
   `ratatui/scrolling-regions` ([`:33`](../../../crates/cyrup-tui/Cargo.toml#L33)), so ratatui's
   feature dispatch (`inline.rs:113`) takes `insert_before_scrolling_regions`, not the `clear()`
   path. — F5
6. **MET — every live collection has a stated bound.** `BashExecution::output_lines` ≤ 2000 lines
   plus an omission counter ([`bash.rs:29`](../../../crates/cyrup-tui/src/bash.rs#L29), [`:152-155`](../../../crates/cyrup-tui/src/bash.rs#L152)), and
   the §5.9 table is true as restated there — **with one bound rewritten rather than re-asserted**:
   `pending_images` is not "cleared on submit"; it is empty in production because nothing in
   production fills it. Nothing new unbounded was introduced. — F6 (+ §5.9)
7. **MET, WITH ONE DELIBERATE AMENDMENT.** The escape hatch still fires from the reader thread
   ([`app/input_reader.rs:193`](../../../crates/cyrup-tui/src/app/input_reader.rs#L193)) and `mark_input_serviced()`
   ([`:52`](../../../crates/cyrup-tui/src/app/input_reader.rs#L52)) still means "serviced" — F3 kept it after the single draw,
   deliberately, so a frame the user never sees is not counted as service
   ([`app/run_action.rs:231-239`](../../../crates/cyrup-tui/src/app/run_action.rs#L231)). The arm order is **not** unchanged:
   `879eb4e`/`TUI-094` inserted the swap arm between input and the tickers, so the invariant is now
   cancel → input → swap → rest. Round 1's half (cancel and input on top) is intact; see §6.

The bar, restated from §5: **a session that runs for hours costs what a fresh one costs** — per
frame, per event, and in retained memory.

**What the six met properties do NOT establish:** that the reported lockup is gone. These are code
properties of the tree, and the report in §1 came from a real terminal. **The row stays OPEN pending
a live re-observation** — see §8.

---

## 8. Open questions for the reporter — Q1 ANSWERED, Q2–Q5 STILL OPEN

**Q1 no longer sets the severity: its `critical` branch is refuted in code.** Q2–Q5 still want a
live answer, and so does the row itself — **the defect has not been re-observed since round 2
landed**, and per this workspace's standing rule a TUI claim is not settled by `TestBackend`. What
round 2 established is that the code properties hold (§7); what nobody has established is what the
terminal now does.

1. ~~**In the locked-up state, do three deliberate `Ctrl+C` presses (about half a second apart) still
   exit and leave a usable shell?**~~ — **ANSWERED FROM CODE 2026-08-19: the `critical` branch is
   REFUTED, so this row is `high`.** Three unserviced `Ctrl+C`/`Ctrl+D` chords hard-exit **from the
   reader thread** — the one context a wedged run loop cannot block, because it is a `std::thread`
   and not the tokio task that wedged ([`app/input_reader.rs:27-204`](../../../crates/cyrup-tui/src/app/input_reader.rs#L27);
   the chord recogniser is [`is_escalate_chord`](../../../crates/cyrup-tui/src/app/input_reader.rs#L176) at `:176`, the exit
   [`hard_exit_from_reader`](../../../crates/cyrup-tui/src/app/input_reader.rs#L193) at `:193`, which drains stdin BEFORE
   restoring the terminal, prints ``cyrup: run loop wedged in arm `{arm}` for {elapsed}`` from
   `ACTIVE_ARM`, kills tracked detached children and exits 130). It is unit-covered by
   [`src/tests/escalation.rs`](../../../crates/cyrup-tui/src/tests/escalation.rs). **The two in-band exits are also both live,
   contrary to the escalation note this row was raised on:** `Ctrl+D` is bound
   ([`keymap.rs:655`](../../../crates/cyrup-tui/src/keymap.rs#L655)) → `Action::Quit`
   ([`app/input.rs:126-129`](../../../crates/cyrup-tui/src/app/input.rs#L126)) → `break` out of `App::run`
   ([`app/run_action.rs:16`](../../../crates/cyrup-tui/src/app/run_action.rs#L16)); `Ctrl+C` is bound
   ([`keymap.rs:656`](../../../crates/cyrup-tui/src/keymap.rs#L656)) → `Action::Clear`
   ([`app/input.rs:219-231`](../../../crates/cyrup-tui/src/app/input.rs#L219), pi's `handleCtrlC` double-tap exit within
   500 ms) and advertised in the startup hint bar from the LIVE keymap
   ([`chrome.rs:80-90`](../../../crates/cyrup-tui/src/chrome.rs#L80)). **So `TUI-088`'s "Ctrl+C has no global binding" — the
   clause that pushed this row to `critical` — is false and has been closed since 2026-08-15.**
   *Still worth confirming live:* that the hatch fires in the **real** wedge, not just the modelled
   one. That is an observation, not a severity question.
2. **Roughly how long, or how many turns, until phase 2 (spinner lag) is noticeable?** Minutes or
   hours; ten turns or a hundred. Any number bounds the growth rate. *(Now doubles as the post-fix
   measurement: if phase 2 no longer arrives at all, this row closes.)*
3. **Does anything make it arrive sooner** — long replies, code-heavy answers, images, many tool
   calls, `/resume`ing a large session? Or is it just elapsed turns regardless of content?
4. **Does the process memory footprint climb visibly** over the session (`top` / Activity Monitor on
   the `cyrup` process)? *(F1 predicted yes on long sessions, and F1 has landed — so a climb that is
   STILL present after `f22efab`+`40821ed` points somewhere the audit did not reach.)*
5. **Does `/new` reset it**, or does a fresh session inside the same process stay slow? *(F1
   predicted it would NOT reset pre-fix; post-F1 it should. **Read this one against `TUI-094`
   first** — until `879eb4e`, `/new` printed its receipt and then wedged the TUI outright with a
   worker hot-spinning at 100 %, which is a different defect that presented identically. Any `/new`
   observation taken before 2026-08-18 is confounded.)*

---

## 9. Cross-references

* Round 1's landed work and QA sign-off — the three closed mechanisms in §2.
* **`TUI-088`** (Ctrl+C does not work in the running TUI) — **CLOSED 2026-08-15 as
  ALREADY-IMPLEMENTED / MIS-DIAGNOSED**, and re-verified line by line at HEAD on 2026-08-19. It is
  cited here because its premise was half of this row's `critical` escalation: see §8 Q1 for the
  live anchors ([`keymap.rs:656`](../../../crates/cyrup-tui/src/keymap.rs#L656) → [`app/input.rs:219-231`](../../../crates/cyrup-tui/src/app/input.rs#L219)
  → [`chrome.rs:80-90`](../../../crates/cyrup-tui/src/chrome.rs#L80)).
* **`TUI-090`** (post-turn whitespace) — **FIXED 2026-08-15**; its `live_floor` × `insert_before`
  machinery is on the per-frame path and is **preserved untouched** by this spec (§6, "Do not
  touch"). Its current anchors are [`app/draw.rs:66-71`](../../../crates/cyrup-tui/src/app/draw.rs#L66) (the flush-synchronized
  release), the `live_floor` field at [`app/mod.rs:206`](../../../crates/cyrup-tui/src/app/mod.rs#L206),
  [`reanchor_inline_region`](../../../crates/cyrup-tui/src/app/backend.rs#L41) at `app/backend.rs:41`, and the guard at
  [`src/tests/live_floor.rs`](../../../crates/cyrup-tui/src/tests/live_floor.rs) — **its own task file
  `bugs/TUI-090-post-turn-whitespace.md` was deleted by `45da9d3` when the fix landed**, so any
  reference to it points at nothing.
* **`TUI-091`** (reasoning blocks never render) — still open, and **it should be re-checked live
  before any further tracing**: the whole chain from provider to paint is proven at HEAD
  ([`app/events_fold.rs:125-126`](../../../crates/cyrup-tui/src/app/events_fold.rs#L125) →
  [`:471-475`](../../../crates/cyrup-tui/src/app/events_fold.rs#L471) → `push_thinking_delta`, with the live block rendered
  ABOVE the answer at [`transcript.rs:1272-1298`](../../../crates/cyrup-tui/src/transcript.rs#L1272)), and F2's cache cannot
  hide it because `push_thinking_delta` bumps `render_generation` on its first statement
  ([`transcript.rs:706`](../../../crates/cyrup-tui/src/transcript.rs#L706)). TUI-090 was fixed the same day TUI-091 was filed,
  so a correctly-rendered block sitting in invisible native scrollback is the leading explanation.
* **`TUI-093`** (the mid-session cursor-position query) — **FIXED 2026-08-17** (`77dca02` +
  `743cad8`), and it lands squarely on this row's per-frame path, so read it before touching `draw`:
  ratatui reached `CrosstermBackend::get_cursor_position` — a blocking `CSI 6 n` round-trip — from
  `insert_before`, from `autoresize` inside every `Terminal::draw`, and from `Viewport::Inline`
  construction, while `crossterm_input_stream`'s reader thread held `event::read`. `InlineBackend`
  ([`app/backend.rs:91`](../../../crates/cyrup-tui/src/app/backend.rs#L91), `with_anchor` [`:100`](../../../crates/cyrup-tui/src/app/backend.rs#L100))
  now **answers** with the anchor cyrup itself set ([`get_cursor_position`](../../../crates/cyrup-tui/src/app/backend.rs#L154)
  at `:154`, re-emitting the `MoveTo` so the answer is true rather than remembered) and carries it
  across the re-wrap ([`rebuild`](../../../crates/cyrup-tui/src/app/backend.rs#L207) `:207`,
  [`reanchor_inline`](../../../crates/cyrup-tui/src/app/backend.rs#L211) `:211`); the process makes exactly ONE cursor query,
  pre-reader-thread and hard-bounded ([`app/crossterm.rs:65-82`](../../../crates/cyrup-tui/src/app/crossterm.rs#L65)); and the
  viewport reconstruction is now **non-fatal** ([`app/draw.rs:78-85`](../../../crates/cyrup-tui/src/app/draw.rs#L78)) instead
  of unwinding ~40 `draw_synchronized()?` sites out of `App::run`. **It was filed retroactively and
  closed in one pass on 2026-08-19**: the row is `07-cyrup-tui.md:470`, with its status-index entry
  at `:316`, and `00-residual-ledger.md:120` records it as **FILED AND CLOSED 2026-08-19 as
  `TUI-093`**. Until that filing it lived only in those two commit subjects and in 16 in-source
  citations across five files (`app/backend.rs`, `app/crossterm.rs`, `app/draw.rs`,
  `terminal_query.rs`, `src/tests/resize_viewport_failure.rs`).
* **`TUI-094`** (the starved session-swap arm) — **FIXED 2026-08-18** (`879eb4e`), and **filed
  retroactively and closed in the same 2026-08-19 pass**: the row is `07-cyrup-tui.md:471`, with its
  status-index entry at `:317`, and `00-residual-ledger.md:121` records it as **FILED AND CLOSED
  2026-08-19 as `TUI-094`**. The id was provisional when this bullet was first written — it is the
  filed id now. It matters here for two reasons. First, it produced *this row's exact symptom* — an
  unresponsive TUI with one worker hot-spinning at 100 % — from a different mechanism: the events
  arm bound `maybe_ev = events.next()`, an IRREFUTABLE pattern, and `Fanout::invalidate`
  ([`subscriber.rs:89-93`](../../../crates/cyrup-session-svc/src/subscriber.rs#L89)) drops every
  sender on a swap, so a dead subscription went permanently `Ready(None)`, matched, and under
  `biased;` starved every arm below it — including the swap arm, which sat LAST. **Any pre-2026-08-18
  observation of "`/new` and then nothing responds" is confounded between the two rows.** Second,
  **the fix violates this file's own §6 "Do not touch"** as it was written: the events arm is now
  refutable ([`app/run.rs:344`](../../../crates/cyrup-tui/src/app/run.rs#L344)) and the swap arm was hoisted to position #3
  ([`:293`](../../../crates/cyrup-tui/src/app/run.rs#L293)), with the rebind extracted to `App::on_session_swapped`
  ([`app/run_arms.rs:138`](../../../crates/cyrup-tui/src/app/run_arms.rs#L138)) and pinned by
  [`src/tests/run_loop_swap_arm_reachable.rs`](../../../crates/cyrup-tui/src/tests/run_loop_swap_arm_reachable.rs). §6 now
  states the amended invariant explicitly.
* Third-party sources consulted (vendored for citation; **`tmp/` is `.gitignore`d since `06eff7a`,
  so these resolve only in a checkout that has them**):
  [`tmp/ratatui-core-0.1.2/src/terminal/inline.rs`](../../../tmp/ratatui-core-0.1.2/src/terminal/inline.rs)
  (both `insert_before` paths),
  [`tmp/ratatui-crossterm-0.1.2/src/lib.rs`](../../../tmp/ratatui-crossterm-0.1.2/src/lib.rs)
  (the `scroll_region_up/down` backend impls, and the `CSI 6 n` `get_cursor_position` TUI-093
  removed from the hot path),
  [`tmp/ratatui-image-11.0.6/src/picker.rs`](../../../tmp/ratatui-image-11.0.6/src/picker.rs)
  (`Picker::new_protocol`, the per-frame encode F7 now caches).
