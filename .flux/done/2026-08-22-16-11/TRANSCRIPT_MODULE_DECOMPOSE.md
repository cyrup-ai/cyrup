---
stage: qa
status: completed
updated: 2026-08-22 16:47
---

# Decompose cyrup-tui src/transcript.rs Into Submodules

## Objective

[`crates/cyrup-tui/src/transcript.rs`](../../crates/cyrup-tui/src/transcript.rs) is **5,452 lines** —
the largest file in the crate after `editor.rs` (3,410) and `markdown/latex.rs` (2,242). Convert it
into a `src/transcript/` module directory whose submodules each own one nameable concern.

**Pure relocation.** No behavior change, no new features, no rewritten logic. Every function body
moves byte-identical; the only edits are `use` headers, module declarations, re-exports, and
visibility keywords.

## Research

### Stack

`Rust`, edition 2024, `rust-version = 1.96`. Workspace lints
([`Cargo.toml:97-101`](../../Cargo.toml)) **deny** `clippy::unwrap_used`, `expect_used`, `panic`,
`indexing_slicing`. `crates/cyrup-tui/src/lib.rs:35` is `#![forbid(unsafe_code)]`. No `rustfmt.toml`
— stock defaults.

### What is actually in the file

`5,452` lines split **3,470 production / 1,982 test**. Seven `#[cfg(test)] mod` blocks (52 `#[test]`
fns) sit at the tail. Verified item spans (doc comments included, 1-indexed, inclusive):

| span | lines | item |
|---|---:|---|
| 1–17 | 17 | `//!` module docs |
| 19–31 | 13 | `use` block |
| 33–244 | 212 | `Entry`, `Rendered` + `impl Rendered`, `ToolRun` |
| 246–261 | 16 | `ResultImage` |
| 263–372 | 110 | `TranscriptView` struct, `RenderCache` struct |
| 374–1354 | 981 | **`impl TranscriptView`** (73 methods) |
| 1356–1378 | 23 | `DEFAULT_MAX_BYTES`, `EXPAND_KEY`, `DEFAULT_IMAGE_WIDTH_CELLS`, `MAX_RASTER_PX`, `HIDDEN_THINKING_LABEL` |
| 1380–1411 | 32 | `thinking_lines` |
| 1413–1539 | 127 | `tool_lines` dispatch, `ImageOpts` + `impl Default` |
| 1541–1612 | 72 | `push_image_fallbacks`, `image_raster_lines`, `decode_result_images` |
| 1614–1889 | 276 | text/wrap primitives (`grapheme_cols` … `body_line`) |
| 1891–2317 | 427 | per-tool renderers (`render_read` … `render_generic`, `edit_header_preview`) |
| 2319–2545 | 227 | arg extraction + compact-read classification |
| 2547–2810 | 264 | result/JSON parsing, truncation + warning footers, formatters |
| 2812–2859 | 48 | `wrapped_height`, `pad_lines` |
| 2861–3215 | 355 | **`entry_lines`** (16 `Entry::*` arms) |
| 3217–3370 | 154 | `collapsed_summary_lines`, `labeled_message_lines`, `group_thousands` |
| 3372–3394 | 23 | `impl Component for TranscriptView` |
| 3396–3469 | 74 | `content_text`, `thinking_text`, `ParsedSkillBlock`, `parse_skill_block` |
| 3471–5452 | 1982 | 7 inline `#[cfg(test)]` modules |

The `impl TranscriptView` block breaks cleanly along lifecycle lines (doc-inclusive spans):

| span | lines | group |
|---|---:|---|
| 375–460 | 86 | `new()` + display-config accessors (`output_pad`, `show_images`, `graphical_images`, `image_width_cells`, `pending`, `streaming`, `has_active`) |
| 461–592 | 132 | `!`/`!!` bash block (`start_bash` … `push_bash_execution`) |
| 593–616 | 24 | `drain_committed`, `chat_has_children` |
| 617–764 | 148 | `push_user`, scroll (`page_up`/`page_down`/`scroll_offset`), assistant + thinking streaming |
| 765–1032 | 268 | tool lifecycle (`push_tool_start` … `tool_expanded`) |
| 1033–1165 | 133 | notices/messages (`push_status` … `push_compaction_summary`) |
| 1166–1192 | 27 | `set_expand_hint`/`expand_key`, `set_cwd`/`cwd` |
| 1193–1353 | 161 | render cache (`bump_render_generation`, `bump_render_tick`, `cached_render`, `content_height`, `lines`) |

Line `1354` is the impl's closing `}`.

### The privacy problem, and the one decision that removes it

`TranscriptView`'s 20 fields are **private** (only `tool_expanded` is `pub`). Rust visibility
inherits *downward*: a private item in module `m` is reachable from `m` **and every descendant of
`m`** — but not from a sibling.

So: **define `TranscriptView` and `RenderCache` in `transcript/mod.rs`, not in a submodule.** Then
`transcript::stream`, `transcript::tool_state`, `transcript::cache`, … are all descendants of
`crate::transcript` and read the private fields with **zero visibility changes**. Putting the struct
in a `view.rs` sibling would force `pub(in crate::transcript)` onto all 20 fields for no gain.

This is exactly what the crate already does: [`src/app/mod.rs:185-206`](../../crates/cyrup-tui/src/app/mod.rs)
declares `struct App<B>` with private fields and splits `impl App<InlineBackend<Stdout>>` across
[`crossterm.rs:38`](../../crates/cyrup-tui/src/app/crossterm.rs),
[`run.rs:29`](../../crates/cyrup-tui/src/app/run.rs),
[`run_action.rs:6`](../../crates/cyrup-tui/src/app/run_action.rs) and
[`run_arms.rs:13`](../../crates/cyrup-tui/src/app/run_arms.rs). Splitting an inherent impl across
files is **established convention here**, not a novelty.

### Keep the layout flat

Sibling modules only (`tests/` is the sole subdirectory). Flat means `pub(super)` uniformly resolves
to "visible throughout `crate::transcript`", which is what every cross-module helper needs. A nested
`tools/` directory would make `pub(super)` inside it mean `pub(in crate::transcript::tools)` and
silently break sibling access — forcing verbose `pub(in crate::transcript)` everywhere. `src/app/`
is flat across 30 files for the same reason.

### External API surface that MUST keep resolving

[`src/lib.rs:86`](../../crates/cyrup-tui/src/lib.rs) is `mod transcript;` (private) and
`src/lib.rs:225-228` re-exports:

```rust
pub use transcript::{
    content_text, parse_skill_block, thinking_text, Entry, ParsedSkillBlock, ResultImage,
    TranscriptView, DEFAULT_IMAGE_WIDTH_CELLS, HIDDEN_THINKING_LABEL,
};
```

`lib.rs` is **not to be edited**. Eighteen files reference `transcript::`. The full set of paths
that must still resolve:

| path | vis | consumers |
|---|---|---|
| `Entry` | `pub` | `app/mod.rs`, `tests/app_global_actions.rs` |
| `Rendered` | `pub` | `app/events.rs`, `app/events_fold.rs`, `app/extension_render.rs`, `app/session_bind.rs` |
| `ToolRun`, `ResultImage`, `ParsedSkillBlock`, `TranscriptView` | `pub` | `lib.rs`, `app/state.rs` |
| `content_text`, `thinking_text`, `parse_skill_block` | `pub` | `app/mod.rs`, `app/event_extract.rs` |
| `DEFAULT_IMAGE_WIDTH_CELLS`, `HIDDEN_THINKING_LABEL` | `pub` | `lib.rs` |
| `entry_lines` | `pub(crate)` | `app/mod.rs` |
| `tool_lines`, `ImageOpts` | `pub(crate)` | `app/draw.rs` |
| `wrap_line` | `pub(crate)` | `chrome.rs`, `markdown.rs`, `settings_selector.rs` |
| `wrapped_height` | `pub(crate)` | `app/draw.rs`, `chrome.rs`, `login_dialog.rs`, `selector.rs` |
| `text_lines_of` | `pub(crate)` | `bash.rs`, `extension_editor.rs`, `model_selector.rs`, `selector.rs`, `startup.rs`, `user_message_selector.rs` |
| `is_ws_grapheme` | `pub(crate)` | `markdown.rs` |

All of these are satisfied by re-exports in `transcript/mod.rs` — no call site changes anywhere.

### Test-module coupling

All seven inline test modules open with `use super::*;`, which today pulls in every private item of
the file. Audit of what they actually reach for beyond the public/`pub(crate)` API:

| private item | refs in tests |
|---|---:|
| `box_lines` | 3 |
| `apply_bg` | 1 |
| `more_lines_hint` | 1 |
| `truncation` | 1 |
| `pad_lines` | 1 |
| `COMPACT_RESOURCE_FILE_NAMES` | 1 |

Only six. Everything else is `TranscriptView`, `Entry`, `entry_lines`, `tool_lines`, `ImageOpts`,
`wrapped_height`, `HIDDEN_THINKING_LABEL` — all re-exported from `mod.rs`. Tests live under
`transcript/tests/`, which is a **descendant** of `crate::transcript`, so `pub(super)` items in
sibling modules stay reachable by path.

### Pre-existing wart this split exposes

Lines `1193–1203` are an orphaned doc paragraph describing `lines()` that got glued onto
`bump_render_generation`'s doc comment (`1204–1208`) — visible at
[`transcript.rs:1193`](../../crates/cyrup-tui/src/transcript.rs). When `lines()` moves to `cache.rs`,
reattach `1193–1203` to `lines()` and leave `1204–1208` on `bump_render_generation`. This is a
doc-placement correction, not a rewrite; do not reword either paragraph.

---

## Required implementation

### Target layout

`crates/cyrup-tui/src/transcript/` (delete `src/transcript.rs`, keep `mod transcript;` in `lib.rs`):

| file | source spans | ~lines | owns |
|---|---|---:|---|
| `mod.rs` | 1–17, 263–372 | ~170 | module docs, `mod` decls, re-exports, `TranscriptView` + `RenderCache` structs |
| `entry.rs` | 33–244 | ~220 | `Entry`, `Rendered` + `impl Rendered`, `ToolRun` |
| `images.rs` | 246–261, 1367–1374, 1541–1612 | ~105 | `ResultImage`, `DEFAULT_IMAGE_WIDTH_CELLS`, `MAX_RASTER_PX`, `push_image_fallbacks`, `image_raster_lines`, `decode_result_images` |
| `content.rs` | 3396–3469 | ~80 | `content_text`, `thinking_text`, `ParsedSkillBlock`, `parse_skill_block` |
| `view.rs` | 375–460, 593–616, 1166–1192 | ~145 | `impl TranscriptView`: `new`, config accessors, `drain_committed`, `chat_has_children`, expand-hint, cwd |
| `bash_block.rs` | 461–592 | ~140 | `impl TranscriptView`: `!`/`!!` bash lifecycle |
| `stream.rs` | 617–764 | ~155 | `impl TranscriptView`: `push_user`, scroll, assistant + thinking streaming |
| `tool_state.rs` | 765–1032 | ~275 | `impl TranscriptView`: tool-run lifecycle |
| `notices.rs` | 1033–1165 | ~140 | `impl TranscriptView`: status/receipt/error/warning/block/skill/custom/summary pushes |
| `cache.rs` | 1193–1353, 3372–3394 | ~195 | `impl TranscriptView`: render cache + `lines()`, and `impl Component for TranscriptView` |
| `layout.rs` | 1614–1889, 2812–2859 | ~330 | `grapheme_cols`, `is_ws_grapheme`, `wrap_line`, `box_lines`, `apply_bg`, `text_lines_of`, `text_lines`, `finalize_block`, `replace_tabs`, `body_line`, `wrapped_height`, `pad_lines` |
| `message.rs` | 1376–1411, 3217–3370 | ~195 | `HIDDEN_THINKING_LABEL`, `thinking_lines`, `collapsed_summary_lines`, `labeled_message_lines`, `group_thousands` |
| `render.rs` | 2861–3215 | ~365 | `entry_lines` |
| `tool_render.rs` | 1358–1365, 1413–1539 | ~145 | `EXPAND_KEY`, `tool_lines` dispatch, `ImageOpts` + `impl Default` |
| `tool_builtin.rs` | 1891–2317 | ~435 | `render_read`/`write`/`edit`/`bash`/`grep`/`find`/`ls`/`extension`/`generic`, `edit_header_preview` |
| `tool_args.rs` | 2319–2545 | ~235 | `StrArg`, `str_arg`, `tool_path_span`, `push_search_path`, `read_line_range`, `key_hint_spans`, `more_lines_hint`, `COMPACT_RESOURCE_FILE_NAMES`, `CompactRead`, `compact_read_classification`, `compact_read_call` |
| `tool_result.rs` | 1356–1357, 2547–2810 | ~275 | `DEFAULT_MAX_BYTES`, `result_text`, `content_blocks_text`, `truncation`, `tnum`, `format_size`, `format_duration`, `push_list_output`, `push_error_body`, `trim_trailing_empty`, `push_read_truncation`, `strip_bash_footer`, `push_{bash,grep,find,ls}_warnings`, `shorten_path` |
| `tests/mod.rs` | — | ~10 | 7 `mod` decls |
| `tests/output_pad.rs` | 3471–3708 | ~240 | relocated `output_pad_tests` |
| `tests/skill.rs` | 3710–3763 | ~55 | relocated `skill_tests` |
| `tests/progressive_commit.rs` | 3765–3855 | ~95 | relocated `progressive_commit_tests` |
| `tests/vertical_rhythm.rs` | 3858–4120 | ~265 | relocated `vertical_rhythm_tests` |
| `tests/rhythm_followup.rs` | 4122–4521 | ~400 | relocated `rhythm_followup_tests` |
| `tests/x_group.rs` | 4523–5069 | ~550 | relocated `x_group_tests` |
| `tests/render_cache.rs` | 5071–5452 | ~385 | relocated `render_cache_tests` |

Largest production module: `tool_builtin.rs` at ~435 — a 12.5× reduction from 5,452, and every file
has a one-sentence charter.

### Step 1 — extract by span, never by retyping

Hand-transcribing 3,470 lines is how a "pure move" acquires a regression. Cut every span
mechanically out of the original **before** deleting it:

```bash
cd crates/cyrup-tui
mkdir -p src/transcript/tests
S=src/transcript.rs; D=src/transcript

sed -n '33,244p'   $S >  $D/entry.rs.body
sed -n '246,261p'  $S >  $D/images.rs.body
sed -n '1367,1374p' $S >> $D/images.rs.body
sed -n '1541,1612p' $S >> $D/images.rs.body
sed -n '3396,3469p' $S >  $D/content.rs.body
sed -n '1614,1889p' $S >  $D/layout.rs.body
sed -n '2812,2859p' $S >> $D/layout.rs.body
sed -n '1376,1411p' $S >  $D/message.rs.body
sed -n '3217,3370p' $S >> $D/message.rs.body
sed -n '2861,3215p' $S >  $D/render.rs.body
sed -n '1358,1365p' $S >  $D/tool_render.rs.body
sed -n '1413,1539p' $S >> $D/tool_render.rs.body
sed -n '1891,2317p' $S >  $D/tool_builtin.rs.body
sed -n '2319,2545p' $S >  $D/tool_args.rs.body
sed -n '1356,1357p' $S >  $D/tool_result.rs.body
sed -n '2547,2810p' $S >> $D/tool_result.rs.body
# impl TranscriptView groups (wrap each in `impl TranscriptView { … }`)
sed -n '375,460p'   $S >  $D/view.rs.body
sed -n '593,616p'   $S >> $D/view.rs.body
sed -n '1166,1192p' $S >> $D/view.rs.body
sed -n '461,592p'   $S >  $D/bash_block.rs.body
sed -n '617,764p'   $S >  $D/stream.rs.body
sed -n '765,1032p'  $S >  $D/tool_state.rs.body
sed -n '1033,1165p' $S >  $D/notices.rs.body
sed -n '1193,1353p' $S >  $D/cache.rs.body
sed -n '3372,3394p' $S >> $D/cache.rs.body
# mod.rs pieces
sed -n '1,17p'      $S >  $D/mod.rs.head
sed -n '263,372p'   $S >  $D/mod.rs.body
# tests
sed -n '3471,3708p' $S >  $D/tests/output_pad.rs.body
sed -n '3710,3763p' $S >  $D/tests/skill.rs.body
sed -n '3765,3855p' $S >  $D/tests/progressive_commit.rs.body
sed -n '3858,4120p' $S >  $D/tests/vertical_rhythm.rs.body
sed -n '4122,4521p' $S >  $D/tests/rhythm_followup.rs.body
sed -n '4523,5069p' $S >  $D/tests/x_group.rs.body
sed -n '5071,5452p' $S >  $D/tests/render_cache.rs.body
```

Then, per file: prepend the `use` header (and the `impl TranscriptView {` wrapper where the span is
method bodies), rename `*.body` → `*.rs`, and `git rm src/transcript.rs`. **Delete the original
last**, after every span is extracted.

### Step 2 — `transcript/mod.rs`

Keep lines `1–17` verbatim as the `//!` header. Then:

```rust
use ratatui::text::Line;

use crate::bash::BashExecution;

mod bash_block;
mod cache;
mod content;
mod entry;
mod images;
mod layout;
mod message;
mod notices;
mod render;
mod stream;
mod tool_args;
mod tool_builtin;
mod tool_render;
mod tool_result;
mod view;

#[cfg(test)]
mod tests;

pub use content::{ParsedSkillBlock, content_text, parse_skill_block, thinking_text};
pub use entry::{Entry, Rendered, ToolRun};
pub use images::{DEFAULT_IMAGE_WIDTH_CELLS, ResultImage};
pub use message::HIDDEN_THINKING_LABEL;

pub(crate) use layout::{is_ws_grapheme, text_lines_of, wrap_line, wrapped_height};
pub(crate) use render::entry_lines;
pub(crate) use tool_render::{ImageOpts, tool_lines};
```

…followed by lines `263–372` verbatim (the `TranscriptView` and `RenderCache` struct definitions,
fields untouched).

`TranscriptView` itself needs no `pub use` — it is *defined* in `mod.rs`, so `crate::transcript::TranscriptView`
already resolves.

### Step 3 — visibility

Exactly one mechanical rule: **every private (bare `fn`/`struct`/`enum`/`const`) free item that
moves out of `mod.rs` becomes `pub(super)`.** Nothing becomes `pub` or `pub(crate)` that was not
already. Because the layout is flat, `pub(super)` in any submodule means `pub(in crate::transcript)`
— reachable by every sibling and by `tests/`.

Items already `pub(crate)` (`is_ws_grapheme`, `wrap_line`, `text_lines_of`, `wrapped_height`,
`tool_lines`, `ImageOpts`, `entry_lines`) keep `pub(crate)` and are re-exported per Step 2.
Items already `pub` (`Entry`, `Rendered`, `ToolRun`, `ResultImage`, `ParsedSkillBlock`,
`content_text`, `thinking_text`, `parse_skill_block`, `DEFAULT_IMAGE_WIDTH_CELLS`,
`HIDDEN_THINKING_LABEL`) keep `pub` and are re-exported per Step 2.

`TranscriptView`'s private methods (`chat_has_children` → `view.rs`, `pending_run_mut` →
`tool_state.rs`, `bump_render_generation`/`cached_render`/`lines` → `cache.rs`) also become
`pub(super)`: `chat_has_children` is called from `stream.rs` and `notices.rs`, and
`bump_render_generation` is called from nearly every mutator across `stream.rs`, `bash_block.rs`,
`tool_state.rs`, `notices.rs` and `view.rs`.

### Step 4 — `use` headers per file

The original's twelve imports (lines 19–31) redistribute as:

| import | files needing it |
|---|---|
| `cyrup_core::Content` | `content.rs` |
| `ratatui::layout::Rect` | `cache.rs` |
| `ratatui::style::Style` | `layout.rs`, `render.rs`, `message.rs`, `tool_render.rs`, `tool_builtin.rs`, `tool_args.rs`, `tool_result.rs` |
| `ratatui::text::{Line, Span}` | `mod.rs` (`Line` only), `layout.rs`, `render.rs`, `message.rs`, `images.rs`, `tool_render.rs`, `tool_builtin.rs`, `tool_args.rs`, `tool_result.rs`, `cache.rs`, `bash_block.rs` |
| `ratatui::widgets::{Paragraph, Wrap}` | `cache.rs` |
| `ratatui::Frame` | `cache.rs` |
| `serde_json::Value` | `entry.rs`, `images.rs`, `tool_state.rs`, `tool_builtin.rs`, `tool_args.rs`, `tool_result.rs`, `tool_render.rs` |
| `unicode_segmentation::UnicodeSegmentation` | `layout.rs` |
| `crate::bash::BashExecution` | `mod.rs`, `entry.rs`, `bash_block.rs`, `cache.rs` |
| `crate::component::Component` | `cache.rs` |
| `crate::image::{image_fallback_text, ImageBlock}` | `images.rs` |
| `crate::theme::UiTheme` | every rendering module + `cache.rs` |

Intra-tree imports use `use super::…` (e.g. `use super::layout::{body_line, box_lines, text_lines};`
in `tool_builtin.rs`). Let `cargo build` drive the exact set — add nothing speculatively; an unused
import is a warning the gate in Step 6 will reject.

### Step 5 — tests

`transcript/tests/mod.rs`:

```rust
mod output_pad;
mod progressive_commit;
mod render_cache;
mod rhythm_followup;
mod skill;
mod vertical_rhythm;
mod x_group;
```

Each relocated file: drop the outer `#[cfg(test)] mod <name> { … }` wrapper and its indentation
(the file *is* the module now), then:

- `output_pad_tests`, `skill_tests`, `progressive_commit_tests`, `render_cache_tests` carry an
  **outer** `#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing,
  clippy::panic)]` — convert it to an inner `#![allow(…)]` at the top of the new file.
- `vertical_rhythm_tests`, `rhythm_followup_tests`, `x_group_tests` already carry an **inner**
  `#![allow(…)]` as their first body line — leave it exactly where it is.
- `vertical_rhythm_tests`, `rhythm_followup_tests`, `x_group_tests` are each preceded by a `///`
  doc block (old lines 3858–3861, 4122–4125, 4523–4528) — convert those to `//!` inner docs at the
  top of the new file. `render_cache_tests` already opens with a `//!` inner doc; keep it.

Then replace `use super::*;` with:

```rust
use crate::transcript::*;
```

plus, only in the files that need them, explicit paths to the six private items:

- `box_lines`, `apply_bg`, `pad_lines` → `use crate::transcript::layout::{apply_bg, box_lines, pad_lines};`
- `more_lines_hint` → `use crate::transcript::tool_args::more_lines_hint;`
- `truncation` → `use crate::transcript::tool_result::truncation;`
- `COMPACT_RESOURCE_FILE_NAMES` → `use crate::transcript::tool_args::COMPACT_RESOURCE_FILE_NAMES;`

Keep each file's own extra imports (`ratatui::style::{Color, Modifier}` in `vertical_rhythm.rs`,
`serde_json::json` in `x_group.rs`, `ratatui::backend::TestBackend` + `ratatui::Terminal` in
`render_cache.rs`).

**Do not author new tests.** These 52 move as-is; assertions, names and bodies are untouched.

### Step 6 — prove it was a pure move

Before the first `cargo` invocation, capture a baseline — this workspace has **no `target/`
directory**, so the first build is cold and slow, and two queued tasks
(`CYRUP_IT_COMPILE_ERRORS.md`, `TEST_FAILURES.md`) indicate the tree may not be green to begin with.
Record `cargo build -p cyrup-tui` and `cargo test -p cyrup-tui` output on `HEAD` **before** touching
anything; any failure present in the baseline is out of scope and stays out of scope.

Then, after the split, diff the item inventory old-vs-new. Only visibility prefixes may differ:

```bash
sig() { grep -hE '^\s*(pub(\([a-z: ]+\))? )?(fn|struct|enum|const|impl|type|mod) ' "$@" \
        | sed -E 's/^\s+//; s/ *\{$//; s/^(pub(\([a-z: ]+\))? )?//' | sort; }
SP=/tmp/claude-0/-home-user-cyrup/dd0049dc-6b86-5b39-a0b6-13c581a23d61/scratchpad
git show HEAD:crates/cyrup-tui/src/transcript.rs > $SP/old.rs
sig $SP/old.rs > $SP/old.sig
sig crates/cyrup-tui/src/transcript/*.rs crates/cyrup-tui/src/transcript/tests/*.rs > $SP/new.sig
diff $SP/old.sig $SP/new.sig
```

Expected diff: only the added `mod …;` declarations and the `impl TranscriptView` repeated once per
split file. Any *missing* line means a span was dropped — fix before proceeding.

Cross-check the test count is conserved:

```bash
grep -rc '#\[test\]' crates/cyrup-tui/src/transcript/tests/*.rs | awk -F: '{s+=$2} END {print s}'   # must be 52
```

## Definition of done

- [ ] `crates/cyrup-tui/src/transcript.rs` no longer exists; `crates/cyrup-tui/src/transcript/` holds 17 production modules + `tests/` with 7 files, per the layout table
- [ ] No production module exceeds ~450 lines; each has a single, nameable concern
- [ ] `crates/cyrup-tui/src/lib.rs` is **unmodified**, and no file outside `src/transcript/` changes — the 18 files referencing `transcript::` compile untouched
- [ ] `TranscriptView`/`RenderCache` are declared in `transcript/mod.rs` with their field visibility exactly as before (19 private + `pub tool_expanded`)
- [ ] The orphaned doc paragraph at old lines 1193–1203 is reattached to `lines()` in `cache.rs`; wording unchanged
- [ ] `cargo build -p cyrup-tui` succeeds with **no new warnings** vs. the recorded baseline
- [ ] `cargo clippy -p cyrup-tui --all-targets` reports no new findings vs. the baseline (workspace denies `unwrap_used`/`expect_used`/`panic`/`indexing_slicing`)
- [ ] `cargo test -p cyrup-tui` shows the same pass/fail set as the baseline — all 52 relocated tests run, none skipped, none added
- [ ] The Step 6 signature diff shows only module declarations and repeated `impl TranscriptView` headers — no dropped or renamed items
