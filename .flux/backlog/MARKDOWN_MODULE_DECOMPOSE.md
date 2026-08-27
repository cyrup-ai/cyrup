---
stage: new
status: done
updated: 2026-08-22 18:31
---

# Finish The markdown/ Package — Carve The LaTeX Prepass, Table Renderer And Syntect Highlighter Out Of markdown.rs

> Identified by the `cyrup-tui` hygiene audit (6-dimension fan-out, adversarially verified).
> **Priority:** low · **Effort:** medium

## Description

`crates/cyrup-tui/src/markdown.rs` is 1,897 lines with NO inline test module (zero `#[cfg(test)]` hits), so it is 1,897 production lines — the third-largest production body in the crate. The `markdown/` directory already exists: `mod latex;` is declared at `markdown.rs:57` and `src/markdown/latex.rs` is 2,242 lines. So this file is already the de-facto mod.rs of a half-formed package. Its public surface is narrow — four `render*` entry points (`:86`, `:97`, `:119`, `:142`) plus `trim_partial_closing_fence` (`:375`) — and `grep -rl 'crate::markdown::' src/tests/` returns 0 files.

**Note on mechanics:** `markdown.rs` + `markdown/latex.rs` is already legal 2018-path style, so submodules can be added by declaring them in `markdown.rs` without moving a byte. Moving to `markdown/mod.rs` is a crate-idiom choice matching `app/` and `transcript/`, not a compile necessity — do not treat the move itself as load-bearing.

**The work.** Move the file to `src/markdown/mod.rs` (`lib.rs:60 mod markdown;` is unchanged, the existing `mod latex;` stays). Keep in mod.rs: the module doc, the `use` block (`:1-52`), the four `render*` entry points + `render_inner` (`:86-183`), and the struct definitions `MdRenderer` (`:436`), `ItemFrame` (`:504`), `CellSpans` (`:519`), `TableCapture` (`:522`). Then, each sibling opening `use super::*;`:

- `markdown/prepass.rs` — `MATH_START`/`MATH_END` (`:184-185`), `latex_prepass` (`:209`), `push_math_placeholder` (`:273`), `chars_range` (`:281`), `line_before_is_blank` (`:286`), `fence_at` (`:302`), `fence_block_end` (`:321`), `code_span_end` (`:349`), `trim_partial_closing_fence` (`:375`), `leading_fence` (`:418`), `is_pure_fence` (`:428`) — the whole `:184-435` block, which calls into `latex::tokenize_block`/`tokenize_inline`/`render_token` (`:240-259`) and nothing else in the file. Natural home next to its only consumer.
- `markdown/walk.rs` — the `impl<'t> MdRenderer<'t>` event fold: `new`..`emit_code_block` (`:533-1187`) and `finish` (`:1452-1485`), minus the table methods. This is the one genuinely cohesive pulldown-cmark fold and stays whole.
- `markdown/table.rs` — `emit_table` (`:1188`), `push_table_row` (`:1412`), `MAX_UNBROKEN_WORD_WIDTH` (`:1536`), `spans_width` (`:1544`), `cell_text` (`:1549`), `longest_word_width` (`:1559`), `trim_cell` (`:1569`), `wrap_cell` (`:1620-1729`).
- `markdown/highlight.rs` — `syntax_set` (`:1742`), `highlight_lines` (`:1750`), `pub(crate) fn highlight_code_lines` (`:1780`), `highlight_inner` (`:1811`), `push_code_span` (`:1853`), `scope_style` (`:1877`). Reached internally from exactly one call site, `emit_code_block` (`:1143`). **`highlight_code_lines` at `:1780` is the file's only `pub(crate)` item and must KEEP `pub(crate)`, not be demoted to `pub(super)`, or every external caller breaks** — and mod.rs must re-export it (`pub(crate) use highlight::highlight_code_lines;`) so existing `crate::markdown::highlight_code_lines` paths still resolve.
- Leave the small shared helpers `row_is_blank` (`:1486`), `is_quote_only_row` (`:1498`), `strip_quote_markers` (`:1513`), `display_width` (`:1531`), `heading_depth` (`:1730`) in mod.rs or a `markdown/util.rs`, since both `walk.rs` and `table.rs` use them.

Other cross-file private items need `pub(super)`, same as the transcript precedent. No signature or behaviour change. Do NOT split `markdown/latex.rs` — despite being larger (2,242 lines) it is ~600 lines of static symbol tables (`SYMBOLS :36` through `PLAIN_WRAPPERS :596`) plus one tokenize→layout→render pipeline, i.e. one cohesive concern. While the file is open, `spans_width` (`:1544`) may be dropped in favour of the shared one if GRAPHEME_TRUNCATION_CANON has already landed `src/text_width.rs`; otherwise leave it.

## Acceptance Criteria

- [ ] `src/markdown.rs` no longer exists; `src/markdown/{mod.rs,latex.rs,prepass.rs,walk.rs,table.rs,highlight.rs}` do, and no new file exceeds ~700 lines
- [ ] `highlight_code_lines` is still reachable as `crate::markdown::highlight_code_lines` from every existing caller (no import edits outside `src/markdown/`)
- [ ] `git diff --stat` on the split commit shows near-balanced added/removed line counts (pure relocation — no rewritten bodies, no changed signatures)
- [ ] `cargo build -p cyrup-tui` emits 0 warnings, `cargo test -p cyrup-tui` passes 1270 tests, `cargo clippy -p cyrup-tui --all-targets` shows only escape_reassembly.rs:972

## Evidence

crates/cyrup-tui/src/markdown.rs (verified 1,897 lines, zero `#[cfg(test)]`): mod latex :57, render entry points :86/:97/:119/:142, MATH_START :184, latex_prepass :209, latex:: calls :240-259, trim_partial_closing_fence :375, MdRenderer :436, ItemFrame :504, CellSpans :519, TableCapture :522, emit_code_block :1143, emit_table :1188, push_table_row :1412, finish :1452-1485, helpers :1486-1531, MAX_UNBROKEN_WORD_WIDTH :1536, spans_width :1544, wrap_cell :1620-1729, heading_depth :1730, syntax_set :1742, highlight_code_lines :1780 (only pub(crate) item), scope_style :1877; src/markdown/latex.rs 2,242 lines with SYMBOLS :36 .. PLAIN_WRAPPERS :596; `grep -rl 'crate::markdown::' src/tests/` = 0
