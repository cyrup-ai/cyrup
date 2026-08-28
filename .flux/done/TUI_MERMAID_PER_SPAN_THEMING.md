---
stage: done
status: completed
updated: 2026-08-28
---

# Theme Mermaid Diagrams By Span Class Instead Of One Flat `md_code_block` Role

> Follow-up to `TUI_MERMAID_DIAGRAM_RENDERING` (shipped 2026-08-28).
> **Priority:** medium · **Effort:** small · Area: Markdown, latex, images, diffs and message rendering

## Objective

A rendered mermaid diagram should be themed per span class the way pi does, instead of drawing
every row in one flat `md_code_block` role.

## The original premise of this task was WRONG — retracted 2026-08-28

This file was written during PR #85 and claimed the only missing piece was a `get_fg` accessor on
`mermaid_text::layout::Grid`, buyable with a ~5-line upstream PR. **That is false on three counts**,
each independently fatal. It was inferred from the field name and the presence of
`paint_node_colors` without tracing the data end to end.

1. **`Grid.fg` is not a class channel.** Its own doc (`mermaid-text-0.57.0/src/layout/grid.rs:287-291`)
   says it is "Empty (all `None`) until the caller paints colors". The only painters are gated on
   `with_color` **and** on the mermaid SOURCE carrying `style` / `classDef`+`class` / `linkStyle`
   directives (`render/unicode.rs:1154`, `:1079`, `:738`; `Graph::node_styles` etc. start empty at
   `types.rs:581-584`). For an ordinary `graph TD; A-->B` fence every cell would return `None`.
   Where it is non-`None` it holds the diagram AUTHOR's chosen RGB, which has no relation to pi's
   semantic classes. Mapping arbitrary user RGB onto those roles is not a mapping that exists.
2. **cyrup cannot obtain a `Grid` at all.** The only code that draws one is
   `render::unicode::render_inner` (`render/unicode.rs:700`) — private, ~570 lines, returns
   `String`, and depends on ~35 private helpers in the same 4794-line module. The public entry
   points (`render` / `render_color`) both return `String`. The `Grid` never escapes.
3. **It would not generalise.** Only 2 of 18 diagram kinds reach that pipeline; the other 16
   (`Sequence`, `Pie`, `Er`, `Class`, `Gantt`, …) return a `String` from their own render module
   (`lib.rs:284-392`), and only 3 of 18 render modules mention `Grid`.

Also corrected: pi distinguishes **six** classes, not four — `border`, `text`, `edge`, `edgeLabel`,
`title`, `none` (`mermaid.ts:38-53`), supplied by `grok-mermaid` as `art.styled: Span[][]` where
each `Span` carries a `cls`. mermaid-text has no equivalent and 0.57.0 is the current release.

**The gap itself is real and unchanged**: `markdown/walk.rs:641-653` renders every diagram row with
one `md_code_block_style()`, and `markdown/mermaid.rs:17-26` documents that as deferred.

**This project does not do upstream PRs.** Any route depending on one is out.

## Route A — classify from the rendered text (recommended)

`mermaid_text::render` already returns the finished character grid as a `String`. Every class pi
distinguishes is derivable from the glyphs plus one structural pass — no `Grid` access, no ANSI
parsing, no dependency change, and it works for all 18 diagram kinds because it operates on output.

1. `markdown/mermaid.rs` — add `pub(crate) enum SpanClass { Border, Text, Edge, EdgeLabel, Title, None }`
   mirroring `mermaid.ts:39-52`, and change `DiagramOutcome::Diagram(Vec<String>)` to
   `Diagram(Vec<Vec<(SpanClass, String)>>)`. Leave the pi gate, the `available_width` check
   (`:214-219`), `warning_line` (`:156-163`) and the `Raw`/`Warned` legs alone — they are
   engine-agnostic and already correct.
2. Add a private `classify(rows: &[String]) -> Vec<Vec<(SpanClass, String)>>`:
   detect closed rectangles by scanning for `┌ ╭ ┏` corners and matching partners along
   box-drawing runs — outline cells are `Border`, non-space glyphs strictly inside are `Text`;
   remaining box-drawing/arrow glyphs are `Edge`; remaining non-space runs outside any box are
   `EdgeLabel`; spaces are `None`. Group adjacent equal-class cells into runs so one `Span` is
   emitted per run, matching the shape of pi's `art.styled` rows.
3. `markdown/walk.rs:641-653` — replace the flat `Line::styled(...)` with `Line::from(spans)`, one
   `Span` per run: `Border -> border_muted`, `Edge -> accent`, `EdgeLabel -> muted`,
   `Title -> accent + BOLD`, `Text -> text`, `None -> plain`. Confirm those roles exist on
   `UiTheme`; add any that do not, beside the existing markdown roles.
4. **Document honestly** in the module doc that classes are INFERRED FROM RENDERED GEOMETRY rather
   than reported by the engine, and name where the inference can be wrong — a label that happens to
   sit inside a box reads as `Text`; a box-drawing glyph used decoratively in a node label reads as
   `Border`.

## Acceptance criteria

- [ ] `SpanClass` has exactly pi's six variants, cited to `mermaid.ts:39-52`
- [ ] `grep -rn "ansi" crates/cyrup-tui/src/markdown/mermaid.rs` returns nothing — no ANSI parsing
- [ ] No change to `Cargo.toml`: this route adds no dependency and forks nothing
- [ ] An unstyled `graph TD; A-->B` fence renders with border, edge and text spans carrying
      DIFFERENT styles — the flat-role regression this task exists to fix
- [ ] The module doc states the inference is geometric and names its failure modes
- [ ] `cargo build -p cyrup-tui --all-targets` — no NEW warnings
- [ ] `cargo clippy -p cyrup-tui --all-targets` — denied-lint count not increased
- [ ] `cargo test --workspace` — no pre-existing test regresses

## Constraints

- No tests: another team owns the suite. No benchmarks.
- Workspace lints deny unwrap_used, expect_used, panic, indexing_slicing; cyrup-tui also has
  forbid(unsafe_code) and deny(clippy::string_slice).
- No upstream PRs. No ANSI parser. No hand-ported layout engine. No silent vendoring.
