# ADR-0005 — cyrup builds the alternate-screen (`fullscreen`) TUI mode

**Status** accepted (decided by default under the parity rule — overridable)
**Date** 2026-08-13
**Decides** OQ-3 (`PARITY-PLAN.md:1427-1435`); area 07's `OQ-07-1` (`07-cyrup-tui.md:1094-1113`); PARITY-GAPS' own open question 8 (`PARITY-GAPS.md:835`)
**Blocks released** batch 30's scope; `TUI-019`; the rendering half of `CFG-021`; tracker `DRIFT-022`; the interim wording that batch 14 ships for `SEAM-051`

---

## Context

### What the question is

`--tui-mode fullscreen` puts pi into an alternate-screen renderer with mouse capture, a scrollbar,
text selection and semantic-prompt navigation. cyrup has none of it. `TUI-019` used to be held at
`low` "as a deliberate ADR-0001 divergence"; the 2026-08-12 repair pass struck that justification
(ADR-0001 does not exist in this workspace — `PARITY-GAPS.md:914`) and re-rated the item `medium` on
consequence, leaving the underlying scope question explicitly unanswered. This ADR answers it.

### False premise in the question as posed

The assignment says to read the implementation at `v0.83.0`. **There is no alternate-screen mode at
`v0.83.0`.** Two-sided:

- `git -C pi ls-tree -r --name-only v0.83.0 -- packages/tui/src` lists 28 files and contains no
  `tui-alt-screen.ts`, no `tui-main-screen.ts`, no `components/scroll-view.ts`, no
  `components/alt-screen-flash.ts`, no `layout.ts`, no `layout-node.ts`, no `stack.ts`.
- `git -C pi grep -niE "alternate screen|altscreen|alt-screen|\\x1b\[\?1049|smcup" v0.83.0 -- packages`
  returns **nothing**.
- The same listing at `v0.84.1` contains all seven of those files, and
  `packages/tui/src/keybindings.ts:44-52` @v0.84.1 declares the eight `tui.altScreen.*` ids that do
  not exist at `v0.83.0` (`keybindings.ts` @v0.83.0 ends its id union at `tui.select.cancel`).

So the true statement is not "pi ships it and cyrup did not port it". It is: **the feature landed
upstream after the tag cyrup ported, and is therefore `upstream-drift`, exactly as `TUI-019`,
`CFG-021`, `SEAM-051` and `DRIFT-022` are already classified.** That matters for one reason only —
it means no v0.83.0 port work was skipped, so nobody's earlier judgement is being overturned here.
It does **not** change the answer: `PARITY-PLAN.md:1329-1373` (§5, and OQ-4) freezes the four
upstream tags and absorbs drift batch-by-batch; freezing a tag defers *when* drift is taken, never
*whether*. The parity rule has no "arrived late" exemption any more than it has an "it is large" one.

A second, smaller premise correction: the assignment describes `tui.altScreen.*` as "eight keybinding
ids that pi ships and cyrup does not", which is right, and describes ADR-0001 as covering "DRAWING
only", which this ADR does not depend on either way — the decision below rests on the parity rule and
on the two trees, not on ADR-0001.

### What the feature actually consists of (upstream, all @v0.84.1)

| # | Behaviour | Upstream evidence |
|---|---|---|
| 1 | Flag `--tui-mode <regular\|fullscreen>`, threaded to the composition root | `cli/args.ts:180-193`, `main.ts:935`, `modes/interactive/interactive-mode.ts:345-352` |
| 2 | Settings keys `tuiMode` (default `regular`) and `fullscreenScrollbar` (default `auto`) | `core/settings-manager.ts:135-136`, getters/setters `:1128-1146`; `docs/settings.md:68-69` |
| 3 | `/settings` rows `TUI mode` and `Fullscreen scrollbar`, the latter documented "no effect in regular mode" | `modes/interactive/components/settings-selector.ts:633-645`; fed from `interactive-mode.ts:4411-4412` |
| 4 | **Live** renderer swap from `/settings` with no restart, preserving focus, `clearOnShrink`, `onDebug`, main-screen render state, extension input listeners and theme bindings | `interactive-mode.ts:795-830` (`switchTuiMode`), stable-reference `Proxy` at `:355-372` |
| 5 | Enter/leave alt screen, autowrap off/on, cursor hide, synchronized output around teardown | `tui-alt-screen.ts:44-54`, `:236-250`, `:252-262`, `:265-288` |
| 6 | Mouse enable sequence, **multiplexer-aware**: button-motion under tmux/zellij/screen, all-motion elsewhere | `tui-alt-screen.ts:48-50`, `:236-247` |
| 7 | A `ScrollView` wrapping the whole transcript, `follow: end`, with `auto`/`always`/`hidden` scrollbar and a 1000 ms transient-hide timer | `components/scroll-view.ts:4-78`; root built at `interactive-mode.ts:869-885`; setting applied at `:1895` |
| 8 | Wheel scrolling with overscroll chaining across nested scroll views | `tui-alt-screen.ts:462-501` |
| 9 | Scrollbar hit-test, hover and thumb drag | `tui-alt-screen.ts:526-604` |
| 10 | Text selection: character/word/line granularity, click-count, edge auto-scroll, highlight, clipboard copy, OSC-8 URL activation on click, right-click paste | `tui-alt-screen.ts:514-524`, `:605-963` |
| 11 | Eight `tui.altScreen.*` keybindings, which **intentionally shadow the unmodified editor bindings in fullscreen only** | `keybindings.ts:44-52`, `:153-179`; routing `tui-alt-screen.ts:420-459` |
| 12 | Semantic-prompt jump, implemented by scanning rendered lines for an OSC 133;A prefix | `tui-alt-screen.ts:56`, `:366-379` |
| 13 | Transient "flash" stack (1000 ms default), and `/copy` forking on renderer type to flash `Copied!` instead of a status line | `components/alt-screen-flash.ts:4-30`; `tui-alt-screen.ts:381-383`, `:896`; `interactive-mode.ts:5957-5962` |
| 14 | Image-protocol lifecycle: iterm2 images suppressed while alt-screen is active and restored on exit; kitty placements deleted on stop and evicted off-screen under byte/count caps | `tui-alt-screen.ts:219-226`, `:284-288`, `:290-350`, caps at `:58-60` |
| 15 | On exit **without** `preserveScreen`, the whole document is repainted into the main screen so quitting fullscreen leaves the transcript in normal scrollback | `tui-alt-screen.ts:265-283` |

`tui-alt-screen.ts` is 1047 lines; `scroll-view.ts` 195; `alt-screen-flash.ts` 51.

### What is true in cyrup today (HEAD `72cd292`)

- No alt-screen renderer, no `tui_mode`: `grep -rn "tui_mode\|tui-mode" crates/ --include="*.rs"` returns nothing.
- Mouse events are parsed by crossterm and then **discarded**: `crates/cyrup-tui/src/app.rs:7202` is
  `Event::Mouse(_) => None`, inside `map_event_on`.
- The interactive shell is a single `Viewport::Inline` region: `crates/cyrup-tui/src/app.rs:52`,
  `:817-819` (`Terminal::with_options`, `TerminalOptions { viewport: Viewport::Inline(height.max(1)) }`),
  with committed history pushed to native scrollback via `Terminal::insert_before`.
- **Committed entries are dropped from memory.** `crates/cyrup-tui/src/transcript.rs:505-511`:
  `drain_committed` does `std::mem::take(&mut self.pending)` (`:510`), and the module doc at `:257-262` states
  entries "are emitted to the terminal's native scrollback with `Terminal::insert_before` and never
  re-rendered inside the viewport". Upstream, by contrast, keeps every message component alive in
  `chatContainer` in **both** modes — which is why `switchTuiMode` can hand the identical component
  set to the new renderer (`interactive-mode.ts:808-822`) and why the alt screen can simply wrap
  `documentContainer` in a `ScrollView` (`:869-885`).
- `scrollbarThumb` already resolves correctly, with pi's `?? selectedBg` fallback:
  `crates/cyrup-tui/src/theme.rs:1032-1037` — the theme half of the feature is ported; only the
  painter is missing, as that file's own comment at `:1011-1017` says.
- cyrup **already enters the alternate screen** elsewhere in the same crate:
  `crates/cyrup-tui/src/startup_selector.rs:20` imports `EnterAlternateScreen`/`LeaveAlternateScreen`
  and `:44`, `:52`, `:62` execute them around the pre-session wizard.
- cyrup emits no OSC 133 anywhere (`grep -rn "\]133\|OSC133" crates/ --include="*.rs"` is empty), and
  `into_stdout` (`crates/cyrup-tui/src/app.rs:6115-6120`) pushes `EnableBracketedPaste` +
  `PushKeyboardEnhancementFlags` but no `EnableFocusChange` and no mouse capture.
- `arboard` is already a dependency and already drives clipboard reads
  (`crates/cyrup-tui/src/keymap.rs:82-89`).

### The mechanism-impossibility argument, tested

It does not survive contact with either tree, and the ADR says so plainly:

- **Alternate screen**: `crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen}` — already
  imported and executed by cyrup at `startup_selector.rs:20,44,52,62`. cyrup has run an alternate
  screen in production since before this question was asked.
- **Full-screen drawing**: `Viewport::Fullscreen` is ratatui's default
  (`ratatui-core-0.1.2/src/terminal/viewport.rs:62-74`) and is strictly *simpler* than the inline
  viewport cyrup already maintains (which needs the `RebuildBackend` / `reanchor_inline_region`
  machinery at `app.rs:650-700` precisely because `Viewport::Inline` is immutable after construction).
- **Scrollbar**: `ratatui::widgets::Scrollbar` + `ScrollbarState`
  (`ratatui-widgets-0.3.2/src/scrollbar.rs:83`, `:106`) render the thumb; `auto`/`always`/`hidden` and
  the 1000 ms transient-hide are application state either way.
- **Mouse**: crossterm parses SGR and legacy mouse reports and delivers `Event::Mouse` — cyrup
  already receives them and throws them away at `app.rs:7202`.

There is exactly **one** place where the convenience API is not a drop-in, and it must be written
down so an implementer does not silently lose behaviour. crossterm's `EnableMouseCapture`
(`crossterm-0.29.0/src/event.rs:321-336`) emits `?1000h ?1002h ?1003h ?1015h ?1006h` unconditionally.
pi emits `?1000h ?1002h ?1004h ?1006h` under a multiplexer and `?1000h ?1002h ?1003h ?1004h ?1006h`
otherwise (`tui-alt-screen.ts:48-49`, selected at `:236-247`). Three differences: crossterm always
turns on any-motion tracking (pi deliberately does not under tmux/zellij/screen, because forwarding
every pointer movement makes multiplexers lag), crossterm adds rxvt `?1015h` (pi does not), and
crossterm never enables focus reporting `?1004h` (pi does, and its alt-screen input handler *depends*
on `FOCUS_OUT` to cancel an in-progress selection — `tui-alt-screen.ts:386-403`). **The implementer
must emit the sequences directly rather than take `EnableMouseCapture`**; crossterm `Command`s are
plain ANSI writers, so this is a `queue!` of literal escapes, not a fork.

The residue after all of that is an application layer — the same conclusion `PARITY-GAPS.md:631`
already reached in its mechanism note. Under the standing rule, an application layer is work.

### What genuinely is new, and is the real cost

1. **Retention.** `drain_committed` (`transcript.rs:505-511`) is a cyrup-only memory strategy with no
   upstream counterpart. Fullscreen needs a retained, scrollable document, so `TranscriptView` must
   keep committed entries (or their rendered lines) when the mode is `fullscreen`. This is the single
   largest structural change and it touches `transcript.rs` and `app.rs`, the two most contended
   files in the plan.
2. **Selection, copy and URL activation** (`tui-alt-screen.ts:605-963`, ~350 lines). Capturing the
   mouse takes the terminal's own selection away from the user, so a renderer that captures the mouse
   owes the user a replacement. ratatui provides none of this.
3. **Scrollbar hit-testing and drag** (`:526-604`) — the widget draws, the interaction does not.
4. **Image lifecycle in the alt screen** (`:220-226`, `:285-350`) — interacts with cyrup's existing
   `ratatui-image` path and its capability detection.

## Decision

**Port it.** cyrup builds the alternate-screen TUI mode, and `--tui-mode fullscreen` eventually
works. Concretely, and in this order:

**A. Ships now, under batch 14 / batch 30a, and does not wait for anything below.**

1. `SEAM-051` (batch 14, unchanged): add `--tui-mode` to `KNOWN_LONG_FLAGS` and
   `KNOWN_VALUE_LONG_FLAGS` (`crates/cyrup/src/cli.rs:757-799`, `:801+`), a `TuiMode { Regular,
   Fullscreen }` value enum on `Cli`, pi's two error diagnostics in `apply_arg_leniency`
   (`crates/cyrup/src/diagnostics.rs:90-152`) with the exact upstream strings from `args.ts:186-191`,
   and the help line in `render_help`.
2. `regular` is accepted and is a **no-op**. `fullscreen` parses successfully and is then declined at
   startup with a cyrup-specific, explicitly **temporary** message that names this ADR — e.g.
   `--tui-mode fullscreen is not built yet in this release (ADR-0005); falling back to regular.`
   It must not be dressed up as a pi diagnostic, must not be phrased as a permanent limitation, and
   must be **deleted** by work unit B-13 below. A grep for that string is the tripwire that the
   interim was cleaned up.
3. `CFG-021`'s settings half (batch 30a): model `tuiMode: TuiMode` and
   `fullscreenScrollbar: ScrollViewScrollbar` in `crates/cyrup-config/src/settings.rs` with pi's
   defaults (`regular`, `auto`) and pi's getter semantics —
   `settings.tuiMode === "fullscreen" ? "fullscreen" : "regular"` (`settings-manager.ts:1129`), so any
   other value degrades to `regular` rather than erroring. **Both keys must round-trip byte-faithfully
   even while the renderer is absent**: a `settings.json` written by pi with `tuiMode: "fullscreen"`
   must survive a cyrup read-modify-write untouched. Losing a user's setting is a defect the interim
   does not license.
4. Until the renderer lands, **omit** the `TUI mode` and `Fullscreen scrollbar` rows from `/settings`.
   Capability-gating a settings row is pi's own idiom, not an invention — `settings-selector.ts:490`,
   `:657`, `:676` omit the image rows when the terminal has no image protocol. Shipping the rows
   before the renderer would ship two lying controls, which is precisely what batch 3's unwired-control
   detector exists to stop.

**B. The renderer — `TUI-019`, unconditional from this date, effort L+.** Fourteen work units:

- **B-1 Retention.** Give `TranscriptView` a retained document mode; keep committed `Entry`s when
  `tuiMode == fullscreen`. Do not change the inline path's behaviour.
- **B-2 A renderer seam.** Introduce cyrup's analogue of pi's `ViewportTUI`
  (`packages/tui/src/tui.ts:322-330`): a trait the inline `App` and the new alt-screen renderer both
  satisfy, with `set_layout_root`-equivalent, `scroll_by`, `scroll_to_top`, `scroll_to_bottom`,
  `flash`. Keep the inline renderer the default.
- **B-3 Terminal setup.** `EnterAlternateScreen` / `LeaveAlternateScreen` around a
  `Viewport::Fullscreen` terminal, autowrap `?7l`/`?7h`, cursor hide, and `?2026h`/`?2026l` around
  teardown, mirroring `tui-alt-screen.ts:236-288`.
- **B-4 Mouse enable.** Emit the literal sequences with pi's multiplexer branch
  (`TMUX`/`ZELLIJ`/`STY` set, or `TERM` starting `tmux`/`screen` → button-motion). **Do not** use
  `crossterm::event::EnableMouseCapture` — see the three deltas above. Enable focus reporting.
- **B-5 Scroll state + scrollbar.** A scroll view over the retained document with `follow: end`,
  overscroll chaining (`tui-alt-screen.ts:489-501`), and `ratatui::widgets::Scrollbar` honouring
  `fullscreenScrollbar` — `always` reserves the rightmost column permanently, `auto` shows it only
  while content exceeds the viewport **and** activity is recent (1000 ms, `scroll-view.ts:46`,
  `:65-70`), `hidden` never.
- **B-6 Wheel routing** (`tui-alt-screen.ts:462-501`).
- **B-7 Scrollbar hover/drag hit-testing** (`:526-604`).
- **B-8 Selection + clipboard + OSC-8 click + right-click paste** (`:514-524`, `:605-963`), reusing
  the existing `arboard` dependency (`keymap.rs:82-89`).
- **B-9 The eight keybindings** (below), including the shadowing rule.
- **B-10 Semantic-prompt navigation.** **Allowed mechanism difference, zero behavioural cost:** pi
  scans rendered lines for `\x1b]133;A` (`tui-alt-screen.ts:56`, `:366-379`) because its renderer only
  has lines; cyrup retains structured `Entry`s (B-1) and must jump by `Entry::User` row index instead.
  Reason: cyrup emits no OSC 133 today, and manufacturing marks purely so they can be re-parsed is a
  strictly worse mechanism for the identical result. Required test: the jump lands on the same row a
  pi user lands on, including the "first match strictly past the current `scrollTop`, in the search
  direction" rule and the no-op when none exists.
- **B-11 Flash stack** (`alt-screen-flash.ts`, 1000 ms default) and the `/copy` fork: flash `Copied!`
  in fullscreen, keep the status line in regular (`interactive-mode.ts:5957-5962`).
- **B-12 Image lifecycle**: suppress iterm2 images while alt-screen is active and restore the saved
  capabilities on exit; delete kitty placements on stop; evict off-screen placements under pi's caps
  (16 images / 32 MB transmitted / 64 MB decoded, `tui-alt-screen.ts:58-60`).
- **B-13 Exit repaint + flag cleanup.** On stop without `preserve_screen`, repaint the document into
  the main screen (`:265-283`); with `preserve_screen` (mode switch), do not. Delete the A-2 interim
  message in the same diff.
- **B-14 Live mode switching** from `/settings` (`interactive-mode.ts:795-830`), and re-add the two
  `/settings` rows omitted by A-4.

**C. The eight keybinding ids**, verbatim from `packages/tui/src/keybindings.ts:44-52` and `:153-179`
@v0.84.1:

| id | default keys | description |
|---|---|---|
| `tui.altScreen.pageUp` | `pageUp` | Scroll viewport up one page |
| `tui.altScreen.pageDown` | `pageDown` | Scroll viewport down one page |
| `tui.altScreen.halfPageUp` | *(none — `[]`)* | Scroll viewport up half a page |
| `tui.altScreen.halfPageDown` | *(none — `[]`)* | Scroll viewport down half a page |
| `tui.altScreen.previousPrompt` | `ctrl+shift+up` | Jump to previous semantic prompt |
| `tui.altScreen.nextPrompt` | `ctrl+shift+down` | Jump to next semantic prompt |
| `tui.altScreen.top` | `home` | Scroll viewport to top |
| `tui.altScreen.bottom` | `end` | Scroll viewport to bottom |

Three rules that are easy to lose and are part of the behaviour, not decoration. (i) The comment at
`keybindings.ts:153` — "These intentionally shadow the unmodified editor bindings in fullscreen
mode" — is normative: in fullscreen, `pageUp`/`pageDown`/`home`/`end` scroll the viewport instead of
moving the editor caret; in regular they keep hitting `tui.editor.pageUp`/`pageDown` etc.
(cyrup's ids at `crates/cyrup-tui/src/keymap.rs:144-149`, `:182-183`). (ii) `halfPageUp`/`halfPageDown`
ship **unbound** and must still appear in the keybindings surface so a user can bind them. (iii) Page
scrolling uses `viewport_height - 4` with a floor of 1 (`PAGE_SCROLL_OVERLAP = 4`,
`tui-alt-screen.ts:57`, `:425`, `:431`); half-page uses `floor(viewport_height / 2)`, floor 1.

**D. Sequencing — what batch 30 grows to.** Batch 30 splits in two; the plan goes to 31 batches.

> **Read with ADR-0009, which also rules on batch 30.** `docs/adr/ADR-0009-tui-fidelity-doc.md`
> decides OQ-7 and concludes that `TUI-FIDELITY.md` contributes **zero** new rows to batch 30 — so
> the presentation tail does not grow and 30a's 21 items are the whole of it. Where ADR-0009 says
> "batch 30's items list is unchanged", it means *unchanged by OQ-7*; the 30a/30b **split** below is
> this ADR's and supersedes ADR-0009's enumeration of a single batch 30 (in particular its trailing
> "`TUI-019`-after-OQ-3", which is now `TUI-019` unconditional, in 30b). The two decisions are
> independent and compatible: OQ-7 changes no count, OQ-3 changes no membership.

- **Batch 30a — TUI presentation** (was "batch 30"). The same 20 items (TUI-002, 004, 010, 012, 015,
  017, 020, 025, 032, 036, 038, 041, N01, N02, N03, N06, N07, N08, N09, DRIFT-041) plus `CFG-021`'s
  settings half = **21 items**, effort L. Its dependency on batch 2 is **discharged by this ADR**;
  remaining deps 3, 6, 16.
- **Batch 30b — the fullscreen renderer.** `TUI-019`, **unconditional**, effort **L+**, decomposing
  into the fourteen units B-1…B-14. Depends on 14 (the flag), 16, and 30a — 30b and 30a both rewrite
  `app.rs` and `transcript.rs`, so they must not run concurrently.
- **`TUI-019` alone under-counts 30b and must be decomposed before 30b is scheduled.** At minimum
  B-1 (retention), B-4 (mouse enable sequences), B-5 (scroll + scrollbar), B-7 (scrollbar drag),
  B-8 (selection/clipboard/URL), B-11 (flash + `/copy` fork), B-12 (image lifecycle) and B-14 (live
  switch) each deserve their own id; expect the area-07 ledger to gain roughly eight rows when that
  decomposition happens. That growth is bookkeeping catching up with reality, not new scope.
- **Not blocked by any of this, and shipping regardless:** `SEAM-051` (batch 14 — today
  `--tui-mode regular`, the flag's own default, makes the binary exit 1) and `CFG-021`'s settings half
  (batch 30a). This ADR governs whether `--tui-mode fullscreen` eventually *works*, not whether the
  flag *parses*.

## Consequences

**Ledger changes — gap-analysis IDs whose severity, kind or scope this decision alters.**

- **`TUI-019`** (`07-cyrup-tui.md:714`, table `:158`) — severity stays **medium** (the consequence has
  not changed and this ADR is not a severity input); kind stays `upstream-drift`; **effort L → L+**;
  the word **"conditional" is struck wherever it qualifies this item** — `PARITY-PLAN.md:206` (batch
  table), `:1190-1194` (batch 30's item list) and `:1212-1214` (batch 30's Risk paragraph, whose
  "if OQ-3 is answered 'no alt-screen mode'" branch is now dead). Scope is now the fourteen units
  B-1…B-14, to be decomposed into ids before 30b is scheduled. Its Fix section's part (b) loses the
  words "Contingent on the decision below".
- **`CFG-021`** (`05-cyrup-config-and-resources.md:683`, table `:152`) — severity stays `low`;
  **effort L → S** for the half this ADR schedules, with the renderer-side consumption reassigned to
  `TUI-019`/B-5. Its "**Impact** — none today; deferred with the fullscreen TUI mode itself" line is
  **wrong under this decision and must be rewritten**: both keys must be modelled and round-trip in
  batch 30a, and the impact today is that a `settings.json` carrying `tuiMode` from pi loses the key
  on a cyrup write. Its `/settings` rows are gated per A-4 and re-added in B-14.
- **`SEAM-051`** (`08-cyrup-session-svc-and-modes.md:493`) — severity, kind, effort and batch all
  unchanged (high · upstream-drift · S · batch 14). One addition: the ADR fixes the interim's
  wording and makes it **removable and grep-able**, and pins its removal to B-13. The phrase "not
  supported in this build" in that item's Fix should read "not built yet", so the message does not
  assert a permanence this ADR has just denied.
- **`DRIFT-022`** (`12-upstream-drift-pi-core.md:98`, `:171`, `:512-518`) — **close the tracker.** Its
  Fix currently reads "Do **not** implement yet" and its Verify "n/a while tracking"; both are
  overturned. Close it as `duplicate-of: SEAM-051` with the resolution recorded as "decided by
  ADR-0005 — port it", and remove it from the tracker list at `00-residual-ledger.md:89` and the
  tracker row at `:287`.
- **`00-residual-ledger.md:246`** (the "TUI / alt-screen mode" family row) — the family is no longer
  awaiting a decision; annotate it "decided by ADR-0005".
- **`PARITY-GAPS.md:631` (VL-P19)** — two corrections. (i) Its **mechanism note is too generous**:
  "ratatui supports the alternate screen and mouse capture natively" is right about the alternate
  screen and about *receiving* mouse events, but crossterm's `EnableMouseCapture` is **not** a
  drop-in for pi's enable sequence (three deltas, evidenced above), so replace the note with
  "ratatui/crossterm supply the alternate screen, full-screen viewport, mouse event parsing and
  scrollbar *rendering*; the enable sequence, the scrollbar interaction, selection, and document
  retention are the application layer." (ii) Its two "**see OQ-8**" / "**under either answer to
  OQ-8**" references mean **PARITY-GAPS' own numbered question 8** (`:835`), which collides with
  `PARITY-PLAN.md` §7's OQ-8 (the ~163 lows / CFG-005 question). Disambiguate both to
  "OQ-3 (`PARITY-PLAN` §7) / `OQ-07-1` / ADR-0005". **This is one instance of a general collision**:
  `PARITY-GAPS.md` §6 carries its own nine numbered questions whose numbers do not match
  `PARITY-PLAN.md` §7's (`PG q3 = OQ-5`, `q4 ⊂ OQ-6`, `q6 = OQ-9`, `q7 = OQ-2`, **`q8 = OQ-3`**,
  `q9 = OQ-1`), and ADR-0003, ADR-0004 and ADR-0011 each hit it independently. The binding convention —
  unqualified `OQ-N` means `PARITY-PLAN` §7; the other list is cited `PARITY-GAPS §6 q<N>` — is
  recorded once in `docs/adr/README.md`.
- **`PARITY-GAPS.md:835`** (open question 8) and **`07-cyrup-tui.md:1094-1113`** (`OQ-07-1`) — mark
  answered by this ADR. `OQ-07-1`'s "**Who decides:** a human, in a document in this workspace" line
  is satisfied; its "Interim rating" paragraph stands, since the rating is unchanged.
- **`PARITY-PLAN.md:1427-1435` (OQ-3)** and **`:242-243`** — answered; batch 2's OQ-3 tracker closes.
- **`theme.rs:1011-1017`** and **`crates/cyrup-tui/tests/theme_fidelity.rs:835`** — the comments
  saying the painter is unported stay accurate until 30b, but should cite ADR-0005 rather than
  reading as an open question.

**New work this decision creates that no item covers yet.** B-1 (transcript retention) has no id in
any area file: it is a change to cyrup-only machinery (`drain_committed`) with no upstream
counterpart, so no drift sweep would ever have produced it. It needs an id in area 07 and a note in
the plan that it contends with batch 16 and 30a in `transcript.rs`/`app.rs`. B-4's three
crossterm-vs-pi mouse-sequence deltas likewise have no id and are the kind of detail that gets lost
inside an L+ item.

**What does not change.** Batches 1-29 are untouched. The inline renderer stays the default in every
build, under every setting, exactly as upstream (`settings-manager.ts:1129` defaults to `regular`,
and `settings-selector.ts:635` calls fullscreen "experimental"). Nothing in this decision authorises
degrading the inline path to make the alt-screen path easier.

## Rejected alternatives

**(a) No-op with an explicit not-supported message — the batch-14 interim made permanent.** Cost:
four normal-path features stay missing forever (no fullscreen, no mouse scroll, no scrollbar, no
jump-to-prompt), and `TUI-019` would have to be reclassified as an accepted divergence — a category
that does not exist here (`gap-analysis/README.md:274-276`; `PARITY-GAPS.md:835`). The only argument for it is size,
and size is a sequencing input, not a scope answer. It also strands the already-ported
`scrollbarThumb` resolution (`theme.rs:1032-1037`) as permanently dead code, and leaves cyrup unable
to ever honour a `settings.json` that pi wrote — a config file that silently means something
different in the two binaries is worse than a missing feature.

**(b) Out of scope, with the reason recorded in the flag's own error text.** Cost: everything in (a),
plus the error text becomes a standing public claim that cyrup is not a pi replacement, which no
evidence in either tree supports — the mechanism exists, is already used in-tree
(`startup_selector.rs:44`), and the residue is ordinary application work. This option is only
defensible against an actual impossibility or a stated project constraint, and there is neither: the
"all-Rust, no system library" constraint (`Cargo.toml:187`) is satisfied by crossterm + ratatui, both
already vendored.

**(c) Port it, but keep it inside a single undifferentiated `TUI-019`.** Cost: an L+ item with
fourteen distinct behaviours and no sub-ids is unreviewable — a reviewer who sees the alt screen open
and the wheel scroll will accept it with no selection, no scrollbar drag, no image lifecycle and
crossterm's wrong mouse enable sequence, and every one of those regressions will be invisible to the
ledger. Rejected in favour of the B-1…B-14 decomposition above.

**(d) Defer the decision to the maintainer.** Cost: batch 30 stays unscoped and the plan cannot be
dated. The maintainer has delegated this and asked not to be blocked on it.

## How to reverse this

**"Do not build a fullscreen TUI mode; `--tui-mode fullscreen` is declined permanently."** That would
require a stated project constraint this ADR could not find — a genuine impossibility, not a cost —
and the maintainer accepting, in writing, that cyrup permanently lacks fullscreen mode, mouse
scrolling, a scrollbar and jump-to-prompt, and that a `settings.json` shared with pi carries a key
cyrup can never honour. On reversal: `TUI-019` is closed as `wontfix` with that behavioural cost
recorded verbatim (not silently closed — `PARITY-PLAN.md:1212-1214`), `CFG-021`'s A-3 round-trip
requirement survives anyway so no user setting is lost, batch 30b is deleted and batch 30a reverts to
being batch 30, and the batch-14 interim message becomes permanent and is reworded from "not built
yet" to name the reversing decision.
