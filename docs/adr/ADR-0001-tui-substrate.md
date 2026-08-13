# ADR-0001 — The TUI substrate: ratatui + crossterm, and exactly what that does not excuse

**Status** accepted (decided by default under the parity rule — overridable)
**Date** 2026-08-13
**Decides** The batch-2 deliverable *"Write ADR-0001 into this workspace or delete every reference to
it"* (`docs/PARITY-PLAN.md:248-249`), and the TUI half of **OQ-6**'s third option — *"write ADR-0001
and the requirement ids into **this** workspace"* (`docs/PARITY-PLAN.md:1461`). It is a
**reconstruction** of a decision the code already practises, not a new one.
**Does NOT decide** **OQ-3** / `OQ-07-1` — whether cyrup ships an alt-screen (fullscreen) TUI mode.
That is ADR-0005's question, and §Decision rule 7 below explains why this ADR cannot answer it.
**Blocks released** Batch 2's verification criterion for the 20 `ADR-0001` citations
(`PARITY-PLAN.md:258-259`) · batch 5 (`editor.rs` line-for-line: `TUI-042`, `TUI-043`, `TUI-044`,
`TUI-048`, `TUI-049`) · batch 6 (`stdin-buffer.ts`: `TUI-045`, `TUI-046`, `TUI-047`, `TUI-050`) ·
batch 30's scope boundary · `TUI-004`'s live-resync half · `TUI-015` · `TUI-N06` / `TUI-N07`'s "decide,
do not patch" · the readable citation `07-cyrup-tui.md` needs for `TUI-019`'s re-rating.

---

## Context

### 1. The document did not exist, and the code cites it as settled

At cyrup HEAD `72cd292` (working tree clean), measured rather than assumed:

- `rg -c 'ADR-0001' crates/` → **20 citations across 10 files**: `crates/cyrup-tui/src/app.rs` (7),
  `crates/cyrup-tui/src/transcript.rs` (4), `crates/cyrup-tui/src/lib.rs` (2), and one each in
  `crates/cyrup-tui/src/{theme.rs, startup.rs}`, `crates/cyrup-tui/Cargo.toml`,
  `crates/cyrup-ext/Cargo.toml`, `crates/cyrup-tui/tests/{render.rs, edit_preview.rs,
  assembled_render.rs}`.
- `rg -o 'ADR-[0-9]{4}' crates/` → 20 × `ADR-0001`, 6 × `ADR-0002`. (The brief's figure of 23 is the
  line count of a slightly different pattern; the accurate number is above.)
- `find . -iname 'ADR-*' -not -path './.git/*'` → **nothing**. `docs/adr/` did not exist until this
  file. `rg -c 'spec/' crates/` → 216 lines, of which exactly **one** cites `spec/architecture`
  (`crates/cyrup-core/src/lib.rs:1`); the rest cite `spec/tui/*`, `spec/gap-analysis/*` and
  `spec/architecture`-adjacent paths that are equally absent.
- `README.md:68` advertises "`spec/architecture/*.md` … ADR-0001 (ratatui), ADR-0002 (WASM)".

`docs/gap-analysis/README.md:268-273` states the governing rule: where a code comment invokes an ADR
id to justify a divergence, it is **an unverifiable claim, not a decision of record**. `:274-276`
adds that there is no accepted-divergence category at all. `docs/gap-analysis/PARITY-GAPS.md:914`
records the absence directly.

> **Two citation corrections, made by re-reading rather than by shifting.** Every document in
> `docs/gap-analysis/` cites those two rules as `README:208-212` and `README:213-215`; at HEAD those
> offsets resolve to blind spot 3's baseline-census bullets. The true lines are `:268-273` and
> `:274-276` — and there is a **third** rule at `:130-135` (severity is never held down by an
> unverifiable justification) that the two-anchor habit loses entirely. ADR-0008 adopts all three as
> the canonical anchors and repoints the seven stale sites. Likewise four documents cite
> `PARITY-GAPS.md:709` as the record that ADR-0001 is
> unreadable; `:709` is a pi-subagents paragraph, and the real line is `:914`. Both are stale
> offsets, not wrong claims — every rule quoted is present and says what is quoted. No verdict in
> this ADR or in the ledger turns on either. Flagged because these are the citations the `TUI-019`
> re-rating rests on, and the next pass should re-resolve rather than propagate them.

The cost is already measured, not hypothetical: `TUI-019` was held at `low` for months on this
citation (`docs/gap-analysis/README.md:273`), and the code itself has begun retracting its own use of
it — `crates/cyrup-tui/src/startup.rs:20-24` now reads *"Earlier revisions headed this section
'Deliberate divergences (ADR-0001)'. No ADR document exists in this workspace, so that citation
asserted an authority nothing here can verify, and it read as permission to stop. It was not."*
`crates/cyrup-tui/src/app.rs:1281` carries the same retraction inline.

### 2. What pi actually does, at the tag cyrup ported from

pi `v0.83.0`, `packages/tui/src/tui.ts` (1719 lines) — a hand-rolled string-array renderer:

- `:64-70` — `export interface Component { … render(width: number): string[]; … }`.
- `:280-283` — `Container.render(width)` concatenates each child's `string[]`.
- `:295` — `export class TUI extends Container`.
- `:1258` — `private doRender()`; `:1373-1399` the line differ (`firstChanged` / `lastChanged`,
  `appendStart`); `:1460-1461` the `firstChanged < prevViewportTop` full-redraw rule; `:1143-1170`
  `expandChangedRangeForKittyImages` / `deleteChangedKittyImages`; `:309` / `:750`
  `MIN_RENDER_INTERVAL_MS = 16` with `renderTimer`/`lastRenderAt` scheduling at `:740-760`.

That whole apparatus is a terminal renderer. At `v0.84.1` it is refactored, not retired: `tui.ts`
drops to 1256 lines with `protected abstract doRender()` at `:372`, and `tui-main-screen.ts` (586)
plus `tui-alt-screen.ts` (1047), `layout.ts`, `layout-node.ts`, `components/{stack,v-stack,h-stack,
scroll-view,alt-screen-flash}.ts` appear. **`git ls-tree -r --name-only v0.83.0 packages/tui/src`
confirms none of those files exists at v0.83.0** — the alternate screen is post-baseline drift, not a
thing ADR-0001 ever diverged from (see rule 7).

In the *same package*, at both tags, sit files that draw nothing:

| upstream file | v0.83.0 | v0.84.1 | draws? |
|---|---:|---:|---|
| `stdin-buffer.ts` | 434 | 434 | no — escape-sequence reassembly |
| `word-navigation.ts` | 117 | 117 | no |
| `fuzzy.ts` | 137 | 137 | no |
| `terminal-colors.ts` | 73 | 73 | no — reply parsing |
| `editor-component.ts` | 74 | 74 | no — an interface |
| `undo-stack.ts` | 28 | 28 | no |

`stdin-buffer.ts` alone holds `isCompleteSequence` (`:29`), the CSI/OSC/DCS/APC completeness tests
(`:84`, `:132`, `:150`, `:168`), the unmodified-Kitty-codepoint decoder (`:184`),
`extractCompleteSequences` (`:192`) and bracketed-paste reassembly (`:23-24`, `:319-371`). And
`tui.ts` itself carries non-drawing behaviour: `:765-771` `handleInput` (renamed
`handleTerminalInput` at v0.84.1 `:819-825`) opens **every** dispatch with
`consumeOsc11BackgroundResponse(data)` / `consumeTerminalColorSchemeReport(data)` and returns early;
`:643-646` and `:678-687` write `\x1b[?2031h` / `\x1b[?2031l`; `:688-694` `queryCellSize` writes
`\x1b[16t`.

### 3. What cyrup actually does, at HEAD

- **Substrate**: `ratatui = "0.30.2"` (`crates/cyrup-tui/Cargo.toml:50`). `rg '^crossterm'
  Cargo.toml crates/*/Cargo.toml` → **zero** direct crossterm dependencies; the only path is
  `pub use ratatui::crossterm` (`crates/cyrup-tui/src/lib.rs:215-217`, whose own comment says
  "never add a direct crossterm dep").
- **The render framework is not ported and is not present.** `crates/cyrup-tui/src/component.rs:1-23`
  defines a *retained* `Component { fn render(&mut self, frame: &mut Frame, area: Rect, theme:
  &UiTheme) }` over ratatui's immediate mode, with the module doc stating "ratatui does the
  cell-level diffing underneath". There is no `render(width) -> Vec<String>`, no `previousLines`, no
  `firstChanged`.
- **Inline viewport, never the alternate screen** — `app.rs:5-6`, `:809-812`, `:1027`, `:1504-1508`,
  `transcript.rs:258-262`. Finished entries are drained to `Terminal::insert_before` and live in the
  terminal's own scrollback. `rg EnterAlternateScreen crates/` hits only the pre-session wizard
  (`startup_selector.rs:20, :44`), never the chat UI. `app.rs:7202` is `Event::Mouse(_) => None`.
- **Extension UI crosses as serializable commands**: `crates/cyrup-ext/Cargo.toml:69-70` — "does NOT
  depend on cyrup-tui … (arch-00 §2.1, ADR-0001 R-ARCH-TUI-014 / ADR-0002 R-ARCH-EXT-010)".
- **The non-drawing behaviours are being ported**, and where they were not, it was a defect the
  ledger caught: `stray_reply.rs:1-7` is an explicit port of `tui.ts`'s OSC-11 guard; alongside it
  sit `terminal_query.rs`, `keyboard_protocol.rs`, `drain.rs`, `tmux.rs`, `terminal_progress.rs`,
  `terminal_title.rs`, `footer_data.rs`.

So the citations encode **three** commitments, all of them live in the code:

1. **A content-sized inline viewport, `insert_before` for committed history, no alternate screen for
   the chat UI.** (7 of the 20 sites: `app.rs:652, :714, :810, :1027, :1504`, `transcript.rs:258,
   :1074`, `tests/render.rs:35`.)
2. **crossterm is reached only through ratatui's version-matched re-export.** (`lib.rs:215-217`.)
3. **`cyrup-ext` never links `cyrup-tui`; extension UI crosses as serializable commands.**
   (`cyrup-ext/Cargo.toml:69-70`.)

### 4. How the carve-out was over-applied, and what it cost

`docs/gap-analysis/README.md:189-196` records the failure in its own words: *"cyrup delegates
rendering to ratatui + crossterm, so pi's hand-rolled `render(width): string[]` framework is out of
scope" is correct — for the drawing layer. It was silently extended to everything living in pi's
`packages/tui/src/tui.ts`, including behaviour that draws nothing.* The measured cost, from the
2026-08-12 repair pass (`07-cyrup-tui.md:1052`): six upstream files, 863 lines, **zero mentions of
any of those six basenames anywhere in `docs/gap-analysis` before that pass** — a grep over all
fifteen files returned nothing, so this is a confirmation rather than an inference. Yield
(`:1054`): **nine items, two of them critical** (`TUI-042`…`TUI-050`), both criticals silent data
loss in the prompt editor on ordinary keystrokes, in shipped code no pass had opened.

The second failure is narrower and generalises further. `TUI-004` originally reasoned that mode 2031
is not enabled, so unsolicited terminal pushes cannot arrive. But cyrup **does** issue the OSC-11
query in production — `StdinTerminalProbe::query_background_color` writes `OSC11_BACKGROUND_QUERY`
(`terminal_query.rs:79, :368`), reached from `crates/cyrup/src/main.rs:1590-1600` at boot — and a
reply arriving after the probe's deadline reaches `event::read()` as keystrokes. `stray_reply.rs`
exists precisely because that happens (`:17-32` documents the observed event sequence, including the
split-at-`ESC` variant). `07-cyrup-tui.md:265` states it: *"Not enabling mode 2031 does not make its
hazards moot — cyrup still issues the OSC-11 query and must handle a late reply."*

---

## Decision

**Adopt ratatui + crossterm as cyrup's TUI substrate. The carve-out that follows from it covers
DRAWING ONLY.** Apply these rules literally; none of them requires judgement.

1. **Do not port pi's render framework.** `Component.render(width): string[]`, `Container`'s
   line concatenation, `doRender`'s line differ, `previousLines` / `firstChanged` / `lastChanged` /
   `maxLinesRendered` / `hardwareCursorRow` bookkeeping, and the Kitty-image changed-range expansion
   (`tui.ts:1143-1170` @v0.83.0) have no cyrup counterpart and must not acquire one. ratatui's
   `Buffer` diff is the counterpart. Port the **output** those functions produce, never the
   functions.

2. **The test, applied before invoking this ADR on any upstream line: *does this line draw?* If it
   does not draw, it is IN SCOPE.** "Draws" means it computes the characters, cells, styles or
   colours that land on screen, or diffs them against the previous frame. It does not mean "lives in
   `packages/tui/`", "is called by the renderer", or "is a terminal concern". Specifically in scope,
   with no further argument needed: input sanitation, escape-sequence reassembly, terminal-reply
   handling, mode negotiation, paste and focus semantics, word navigation, undo grouping, the kill
   ring, fuzzy ranking, prompt history, and render **scheduling policy**.

3. **Ask what the code SENDS, not only what it ENABLES.** A capability cyrup declines to turn on does
   not neutralise a query cyrup still writes. If cyrup emits a sequence that solicits a reply, the
   reply-handling path is in scope — including a reply that arrives after the probe that asked for it
   gave up. This is not TUI-specific; apply it to any protocol cyrup speaks.

4. **A mechanism difference that costs behaviour stays on the backlog as work.** Naming this ADR in a
   source comment, a severity justification, or a `Fix` does not close an item and does not lower a
   rating. Never write "deliberate ADR-0001 divergence"; write the mechanism difference, then file the
   behavioural residue with its consequence. There is no accepted-divergence category
   (`gap-analysis/README.md:274-276`), and this ADR does not create one.

5. **Keep the three commitments.** (a) Content-sized inline viewport, `insert_before` for committed
   history, no alternate screen in the chat UI *as the default path* — subject to ADR-0005.
   (b) crossterm only via `ratatui::crossterm` (`lib.rs:215-217`); adding a direct `crossterm`
   dependency to any crate requires overturning this ADR, because a version skew between ratatui's
   backend and a direct dep silently breaks raw-mode and key decoding. (c) `cyrup-ext` must not
   depend on `cyrup-tui`; extension UI crosses as serializable commands. **(c) is a layering rule,
   not a behaviour waiver** — every `ExtensionUIContext` method must still reach the screen through
   that boundary.

6. **Citation hygiene.** An `ADR-NNNN` reference in source must resolve to a readable file under
   `docs/adr/`. Do not add new `R-NN-NNN` or `spec/…` ids: 216 lines in `crates/` cite `spec/` paths
   that do not exist, and one unreadable citation already cost a severity rating. Where a comment
   currently cites `ADR-0001` for a **behavioural** claim rather than one of the three commitments
   above, that comment is wrong and the claim is unbacked.

   **This rule is the TUI application of a project-wide one, and it defers to it.**
   `docs/adr/ADR-0008-requirement-ids-and-sdk-surface.md` §A governs all ~2 195 citation tokens: it
   establishes that they carry no authority, closes the `R-NN-NNN` namespace, and requires the
   **full path** form (`docs/adr/ADR-0001-tui-substrate.md`) rather than the bare token `ADR-0001`,
   because the bare token is now ambiguous between the lost original and this file. The per-site
   triage of the 20 `ADR-0001` sites below **is** ADR-0008 §A.5 performed for this ADR's subject;
   nobody needs to repeat it. Where the two ever disagree on citation policy, ADR-0008 wins.

7. **This ADR does not decide the alternate screen.** `TUI-019` / OQ-3 / `OQ-07-1` belong to
   ADR-0005. Two facts constrain that decision and are settled here: `tui-alt-screen.ts` does not
   exist at v0.83.0, so an alt-screen mode cannot be a "divergence" from the ported baseline in any
   sense — it is `upstream-drift`; and ratatui supports the alternate screen and mouse capture
   natively (`startup_selector.rs:44` already enters it), so the substrate is not the obstacle and
   cannot be cited as one. Whatever ADR-0005 decides, rule 4 applies to the residue.

### Borderline calls, already applied — do not re-derive

**IN SCOPE (does not draw).** `stdin-buffer.ts` in full; `word-navigation.ts`; `undo-stack.ts`'s
coalescing and snapshot payload; `terminal-colors.ts`; `fuzzy.ts`; every `editor-component.ts` member;
`tui.ts`'s `handleInput`/`handleTerminalInput` reply guards (`:765-771` @v0.83.0); mode-2031
negotiation and its listener fan-out; `queryCellSize`; `MIN_RENDER_INTERVAL_MS` coalescing —
**scheduling when to draw is not drawing** (see `TUI-015`); the `ExtensionUIContext` methods.

**OUT OF SCOPE (mechanism-N/A, nothing to port).** The five renderer internals named in rule 1;
`structuredClone` in `undo-stack.ts` (`Vec<Vec<char>>: Clone` is already deep); `stdin-buffer.ts`'s
`EventEmitter` plumbing (`:20`, `:265-274`) — the transport is `tokio::sync::mpsc` at `app.rs:7126`;
`StdinBufferOptions` (`:257-263`) *as a configuration object* — **but its 10 ms default's behaviour is
in scope and is `TUI-045`**; `flush()`/`clear()`/`getBuffer()`/`destroy()` (`:400-433`) as public API.
Each of these is recorded because an unstated exclusion is invisible to the next pass
(`gap-analysis/README.md` blind spot 6).

---

## Consequences

**Ledger — severity / kind / scope changes to record.**

- **`TUI-019`** — the `low` this ADR was cited for is **permanently struck**, and the repair pass's
  re-rating to `medium` (kind `upstream-drift`, effort `L`+) is **confirmed and citable from this
  file**. `07-cyrup-tui.md:70` still carries the superseded sentence "Severity stays low as a
  deliberate ADR-0001 divergence" in its status row; `:158` and `:718-726` already carry the
  correction. This ADR does not close the item, does not lower it, and does not answer its open
  question. Its two separable halves — **`SEAM-051`** (`--tui-mode regular`, the flag's own default,
  makes the binary exit 1) and **`CFG-021`** (`tuiMode` / `fullscreenScrollbar` modelled nowhere) —
  are **not** substrate questions under rule 2 (a CLI flag and two settings keys draw nothing) and
  ship on their existing schedule regardless of ADR-0005.
- **`TUI-042`, `TUI-043`, `TUI-044`, `TUI-045`, `TUI-046`, `TUI-047`, `TUI-048`, `TUI-049`,
  `TUI-050`** — confirmed in scope by rule 2, with no substrate defence available to any of them.
  Ratings stand (two critical, four medium, three low). Batches 5 and 6 own them.
- **`TUI-004`** — confirmed in scope by rule 3, both halves. The `/reload` re-theme half is ordinary
  work; the mode-2031 half is a genuine crossterm mechanism gap (no event type for
  `CSI ? 997 ; N n`), so under rule 4 the behavioural cost — a mid-session dark/light flip does not
  re-theme — **stays filed**, and `theme.rs:1483-1492`'s rationale is a mechanism note, not a
  closure. Note the second-order effect rule 3 makes explicit: because cyrup declines the
  notification but still sends OSC-11, the reply path (`stray_reply.rs`) was mandatory, not optional.
- **`TUI-015`** — resolved as **in scope** by rule 2's scheduling clause. `MIN_RENDER_INTERVAL_MS`
  (`tui.ts:309` @v0.83.0, `:343` @v0.84.1) decides *when* to draw; it does not draw. Kind stays
  `cyrup-original`, severity `medium`. ratatui's cell diff reduces the per-frame write cost but not
  the per-frame layout/markdown/raster cost, which is what the item measures.
- **`TUI-N06`, `TUI-N07`** — these ARE true consequences of commitment 5(a): rows handed to
  `insert_before` are in the terminal's own scrollback and cannot be mutated. Rule 4 therefore
  **strikes option (A)** from both Fixes (`07-cyrup-tui.md:964` "accept it and record it in `lib.rs`'s
  ADR-0001 notes"; `:978` "same ADR-0001 family"). An in-source ADR note is not a closure and never
  was. Both stay open at `low`/`L` with (B) or (C) to be chosen; `TUI-N07`'s "cheapest honest
  improvement" (a real session-boundary rule) is available immediately and is not contingent on
  ADR-0005.
- **`TUI-014`, `TUI-029`, `TUI-030`, `TUI-033`** — commitment 5(c) is a layering rule under rule 5;
  it does not excuse `setWidget` / `setHeader` / `setFooter` / `setAutocompleteProvider` /
  `setEditorComponent` being delivered into fields nothing reads. All four stay open at their current
  ratings, and the WIT-world work they need is in scope.
- **`TUI-N01`, `TUI-017`, `TUI-036`** — the half-block raster rationale at `07-cyrup-tui.md:578`
  ("that ADR-0001 rationale is sound and orthogonal") is **upheld**: `ratatui-image`'s fallback is a
  drawing decision and rule 1 covers it. The *capability gate* is not — a terminal with no image
  protocol must take the text-fallback branch. No rating changes; the ADR only removes an ambiguity
  the Fix would otherwise have to argue.
- **`TUI-020`, `TUI-039`, `TUI-040`, `TUI-051`** — for the record, none of these was ever substrate:
  OSC-8 emission, `$COLUMNS`/`$LINES` fallback, an escape-write log and a keybinding-name migration
  all pass rule 2 trivially. Listed so no future pass re-raises the defence.

**Source comments to correct** (each currently asserts an authority this ADR now supplies, but three
assert it for a *behavioural* claim, which rule 6 makes wrong): `transcript.rs:59` (hide/thinking
freeze — that is `TUI-N06`, not an ADR grant), `app.rs:1281` (already self-retracted, keep the
retraction and point it here), `theme.rs:1492` (keep as a mechanism note, drop any reading of it as a
closure). The remaining 17 citations resolve correctly against the three commitments and need only a
path to this file.

**Batches.** Batch 2 can now satisfy its own verification line
(`rg 'ADR-0001|spec/architecture|R-[0-9]{2}-[0-9]{3}' crates/` returning only resolvable references)
for the `ADR-0001` third of it — 20 citations, 10 files. Batch 5 (`editor.rs` line-for-line) and
batch 6 (`stdin-buffer.ts`) open with the substrate defence formally unavailable, which is the point:
`07-cyrup-tui.md`'s blind spot 9 names `editor.rs` read against `components/editor.ts` at v0.83.0 as
the highest-value remaining target in the area, and rule 2 is what makes that read mandatory rather
than optional. Batch 30 gets its boundary: presentation fidelity and drawing are substrate; every
non-drawing file under `packages/tui/src` is ordinary parity work.

**Standing instruction for future sweeps.** When a sweep excludes a `packages/tui/src` file, it must
record the exclusion with the rule-2 test result ("reads X, draws nothing, in scope" or "computes
cells, mechanism-N/A"). An unstated exclusion is invisible to every later pass — that is exactly how
863 lines and nine items stayed unseen.

---

## Rejected alternatives

- **Delete every `ADR-0001` reference instead of writing the ADR** (batch 2's stated alternative,
  `PARITY-PLAN.md:248`). **Cost:** the three commitments become undocumented convention. Nothing then
  stops a crate adding a direct `crossterm` dependency that skews against ratatui's backend, or
  `cyrup-ext` linking `cyrup-tui` and collapsing the extension boundary — and 20 comments explaining
  *why* the viewport is content-sized lose their referent, so the next author re-derives or breaks
  the `insert_before` invariant. Deleting the citations removes the evidence, not the decision.
- **Port pi's renderer faithfully** — a Rust `render(width) -> Vec<String>` plus the line differ, and
  drive the terminal directly. **Cost:** duplicates ratatui's `Buffer` diff with a hand-written one,
  and re-implements `expandChangedRangeForKittyImages`, `hardwareCursorRow` tracking and the
  viewport-top full-redraw rule (`tui.ts:1143-1170`, `:1373-1461` @v0.83.0) — roughly 400 lines of
  the highest-risk code in the upstream tree — to reach output no user can distinguish from
  ratatui's. It also forfeits `TestBackend`, which every `crates/cyrup-tui/tests/*` file depends on.
  This is the one place the language genuinely forces a mechanism difference, and taking it back
  would buy nothing behavioural.
- **Keep the broad reading — everything under `packages/tui/src` is substrate.** **Cost:** measured,
  not estimated: 863 lines across six files unread by any pass, nine items, two critical, both silent
  data loss in the prompt editor on ordinary keystrokes (`07-cyrup-tui.md:1052-1054`). One more pass under
  the broad reading and `editor.rs` — where `TUI-042`/`043`/`044`/`049` were all found *from the
  outside* — stays unopened.
- **Write ADR-0001 as an accepted divergence that also covers the alternate screen, mouse and
  scrollbar** (i.e. make `TUI-019` go away with a document). **Cost:** contradicts
  `gap-analysis/README.md:274-276` and `PARITY-GAPS.md` head-on; and it is factually unavailable —
  `tui-alt-screen.ts` does not exist at v0.83.0, so there is nothing at the ported baseline to have
  diverged *from*. It would also be false about the substrate, which supports both natively.
- **Record the reconstruction but defer the boundary to the maintainer.** **Cost:** the boundary is
  the entire operative content. Deferring it leaves batches 5, 6 and 30 unscopeable and reproduces
  the exact failure this batch exists to end — a decision nobody made, encoded as a severity.

---

## How to reverse this

> *"The substrate carve-out is wider (or narrower) than drawing — here is the line I want instead."*

To widen it, the maintainer must name the non-drawing behaviours it now covers and accept their cost
in writing: at minimum `TUI-042`…`TUI-050` (two critical), which are the items rule 2 restores.
To narrow it — i.e. to port pi's renderer — batch 30 is re-sized around ~400 lines of differ, the
`ratatui-image` fallback and `TestBackend` coverage are re-planned, and `crates/cyrup-tui/tests/*`
is rewritten against a new backend. Either way rule 4 survives unless the no-accepted-divergence rule
itself is overturned, which is a project-level change, not a TUI one.
