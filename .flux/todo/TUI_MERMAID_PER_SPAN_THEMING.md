---
stage: todo
status: pending
updated: 2026-08-28
---

# Theme Mermaid Diagrams By Span Class Instead Of One Flat `md_code_block` Role

> Follow-up to `TUI_MERMAID_DIAGRAM_RENDERING` (shipped 2026-08-28).
> **Priority:** medium · **Effort:** small · Area: Markdown, latex, images, diffs and message rendering

## Objective

pi themes four mermaid span classes — `border` -> `borderMuted`, `edge` -> `accent`,
`edgeLabel` -> `muted`, `title` -> bold `accent` (`components/mermaid.ts:38-56`). cyrup currently
renders the whole diagram in the single `md_code_block` role. This closes that gap.

## Why this is small, and why it was deferred rather than bodged

The shipped engine, `mermaid-text`, does **not** bake colour into a string the way the rejected
`mermansi` did. It renders into `mermaid_text::layout::Grid`, which keeps per-cell structure:

```
pub struct Grid {
    cells:     Vec<Vec<char>>,          // pub fn get(col, row) -> char   <- already public
    fg:        Vec<Vec<Option<Rgb>>>,   // private, no accessor           <- the only gap
    bg:        Vec<Vec<Option<Rgb>>>,
    hyperlink: Vec<Vec<Option<u32>>>,   // OSC 8, per cell
    ...
}
```

`Grid` is `pub` (`layout/mod.rs:10 pub use grid::Grid`) and `Grid::get` is `pub`. `fg` escapes
only through `Grid::render_with_colors`, which bakes ANSI. So the missing piece is one accessor.

**Do NOT write an ANSI-to-`Span` parser for this.** That was considered and rejected: it couples
cyrup to the engine's palette instead of `UiTheme`, and it is strictly more code than the accessor.

## Subtasks

1. **Upstream** a `pub fn get_fg(&self, col: usize, row: usize) -> Option<Rgb>` (and, if cheap,
   `get_bg`) to `mermaid-text` — ~5 lines beside `Grid::get` (`layout/grid.rs:566`). Repo:
   `github.com/leboiko/markdown-reader`, crate at `crates/mermaid-text`. Record the PR link here.
   If upstream stalls, vendoring the crate (MIT, ~23k loc) is the sanctioned fallback — a review
   cost, not an engineering one. Do not fork-and-patch privately without recording why.
2. **Drive the layout from cyrup** rather than `mermaid_text::render`, so a `Grid` is in hand:
   `detect::detect` -> the matching `parser::*::parse` -> `layout::*` -> keep the `Grid`. Confine
   this to `crates/cyrup-tui/src/markdown/mermaid.rs`; the module's `DiagramOutcome`, `warning_line`
   and the `art.width > available_width` check (`mermaid.ts:76`) are engine-agnostic and must not move.
3. **Map `Rgb` -> role.** Walk the grid per row, group runs of equal `fg`, and emit one
   `ratatui::text::Span` per run styled from `crate::theme`. Map onto pi's four assignments; the
   engine paints node borders (`paint_box_border_fg`), edge paths (`paint_fg_path`) and node
   interiors (`paint_node_colors`) distinctly, which is what makes the classes recoverable.
4. **Keep the colourless path** as the fallback when `get_fg` returns `None` for every cell, so a
   diagram from a code path that never painted still renders.
5. While in here: `Grid` also carries a per-cell hyperlink index. cyrup already has
   `crate::osc::LinkSink`. Note in the module doc whether mermaid `click` directives should feed it,
   but do NOT implement that here — file it separately if wanted.

## Acceptance criteria

- [ ] A mermaid diagram renders with at least two distinct theme roles visible in the emitted spans
- [ ] `grep -rn "ansi" crates/cyrup-tui/src/markdown/mermaid.rs` shows no ANSI parsing was added
- [ ] The `available_width` fallback, the mode gate and the warning line all still behave as
      `TUI_MERMAID_DIAGRAM_RENDERING`'s acceptance criteria specified
- [ ] `cargo clippy -p cyrup-tui --all-targets` — denied-lint count not increased
- [ ] `cargo test --workspace` — no regression

## Constraints

- No tests are to be written for this task; another team owns the test suite.
- Workspace lints deny unwrap_used, expect_used, panic and indexing_slicing; cyrup-tui also has
  forbid(unsafe_code) and deny(clippy::string_slice).
