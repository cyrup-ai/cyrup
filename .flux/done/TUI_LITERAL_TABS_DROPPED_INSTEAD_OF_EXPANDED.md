---
stage: done
status: completed
updated: 2026-08-28
---

# Port `normalizeTerminalOutput`'s Per-Line Pass So Tabs Expand To Three Spaces Instead Of Vanishing

> Identified by the `cyrup-tui` ↔ `pi` port audit (fan-out survey, adversarially verified).
> **Priority:** high · **Kind:** divergent-behaviour · **Area:** Rendering, layout, terminal and scrolling
>
> **Scope note.** This task absorbs the separately-surveyed `THAI_LAO_SARA_AM_OUTPUT_NORMALIZATION`
> gap. It is the same missing hook — pi does both jobs in one function — so it is ported whole here
> rather than filed twice.

## Objective

Tool output that contains literal tabs must keep its column alignment. Today `a\tb` reaches the
screen as `ab`: the tab survives cyrup's sanitizer (correctly, pi keeps it too) and is then
**deleted** by ratatui's cell writer, which filters control-character graphemes. Any tool that is
not `read`/`write`/`diff` is affected — `grep`/`rg`, `ls -l`, `git diff --stat`, TSV or `column -t`
output, `go test`, and everything an extension tool returns. Assistant messages are unaffected
(they go through `markdown::render_inner`), which makes the inconsistency read as a bug rather than
a limitation. The same hook also carries pi's Thai/Lao repaint-artifact guard.

## Upstream reference

- [`packages/tui/src/utils.ts:368-402`](../../tmp/pi/packages/tui/src/utils.ts) —
  `normalizeTerminalOutput` walks each rendered line and does two things outside escape sequences:
  it decomposes **U+0E33** (THAI CHARACTER SARA AM) → `U+0E4D U+0E32` and **U+0EB3** (LAO VOWEL SIGN
  AM) → `U+0ECD U+0EB2` (`:374-390`), then emits `"   "` for every tab (`:379-402`). Its own doc
  comment gives both reasons: "Visible tabs are expanded to the fixed width used by layout so
  terminal tab stops cannot wrap a logical line", and (`:368-374`) "Some terminals render
  precomposed Thai/Lao AM vowels inconsistently during differential repaint. Their compatibility
  decompositions have the same cell width but avoid stale-cell artifacts in terminal renderers."
- [`packages/tui/src/tui.ts:1119-1128`](../../tmp/pi/packages/tui/src/tui.ts) — `TuiBase.applyLineResets`
  applies it to **every** rendered line of both renderers (`tui-main-screen.ts:265`,
  `tui-alt-screen.ts:1290` call it on their full line array each frame).
- [`packages/tui/src/components/text.ts:61`](../../tmp/pi/packages/tui/src/components/text.ts) —
  a second, independent layer: `const normalizedText = this.text.replace(/\t/g, "   ");` runs on
  every `Text` component, which is what generic tool output is wrapped in
  (`tool-execution.ts:155 return new Text(text, 0, 0);`, and `:72`/`:328` `this.contentText`).
- Measurement agrees with the three-space figure:
  [`utils.ts:174-176`](../../tmp/pi/packages/tui/src/utils.ts) `graphemeWidth` returns **3** for
  `"\t"`, `:244-246` `visibleWidth` pre-replaces tabs with three spaces, `:104-113`
  `truncateFragmentToWidth` charges 3 columns per tab. Widths for SARA AM need no change:
  `graphemeWidth` adds +1 (`:213-215`), which is what `unicode-width` already gives.
- pi's per-tool `replaceTabs` ([`core/tools/render-utils.ts:31-33`](../../tmp/pi/packages/coding-agent/src/core/tools/render-utils.ts))
  covers only read/write/diff — every other tool relies on the two layers above. And
  [`utils/shell.ts:144-174`](../../tmp/pi/packages/coding-agent/src/utils/shell.ts)
  `sanitizeBinaryOutput` deliberately **keeps** 0x09.

## Current state in cyrup-tui

| piece | where | what it does / does not do |
|---|---|---|
| the ported half | [`transcript/layout.rs:246-256`](../../crates/cyrup-tui/src/transcript/layout.rs) | `replace_tabs(text) -> text.replace('\t', "   ")` — a correct port of pi's per-tool `replaceTabs`, wired into exactly the call sites pi wires it into: `transcript/tool_builtin.rs:47,97` and `layout.rs:276` (`body_line`). **Do not redo this.** |
| the missing lower layer | [`transcript/layout.rs:226-236`](../../crates/cyrup-tui/src/transcript/layout.rs) | `text_lines` is the port of `Text.render` (`text.ts:60-87`). It builds `Span::styled(logical.to_string(), style)` straight from the source string — `text.ts:61`'s replacement is absent, and there is no normalization hook of any kind. |
| generic tool output | [`transcript/tool_builtin.rs:409-426`](../../crates/cyrup-tui/src/transcript/tool_builtin.rs) | `render_generic` pushes `Line::styled(l.to_string(), theme.tool_output_style())` for the pretty-printed args and for every result line — no expansion. |
| extension tool output | [`transcript/tool_builtin.rs:390-407`](../../crates/cyrup-tui/src/transcript/tool_builtin.rs) | `render_extension` does the same for the extension's `renderCall`/`renderResult` text. |
| sanitizer | [`ansi.rs:42`](../../crates/cyrup-tui/src/ansi.rs) | returns `true` for `code == 0x09`, i.e. tabs pass through — faithful to `sanitizeBinaryOutput`. **Correct; do not change.** |
| the sink that deletes them | vendored `ratatui-core-0.1.2/src/text/span.rs:314` and `buffer/buffer.rs:351` | both filter `.filter(\|g\| !g.contains(char::is_control))`. U+0009 is `Cc`, so the grapheme is dropped rather than rendered. |
| width | [`text_width.rs:20-22`](../../crates/cyrup-tui/src/text_width.rs) | `str_width(s) = Span::raw(s).width()` → **0** for a tab, against pi's 3. |
| wrap | [`transcript/layout.rs:43-60`](../../crates/cyrup-tui/src/transcript/layout.rs) | `wrap_line` classifies `\t` as whitespace via `is_ws_grapheme` at zero width, so wrap points disagree with upstream on any tab-bearing line. |
| Thai/Lao | nowhere | `grep` across `crates/` for `0E33`/`0EB3`/`0E4D`/`0ECD`, the literal characters and the names returns **zero** hits. `layout.rs:186-236` (`box_lines`/`text_lines_of`/`text_lines`) and `app/draw.rs` pass `Line<'static>` straight into ratatui's buffer. |
| existing coverage | [`transcript/tests/x_group.rs:122-137`](../../crates/cyrup-tui/src/transcript/tests/x_group.rs) | exercises the read/write path only. |

## Subtasks

1. **`crates/cyrup-tui/src/transcript/layout.rs`** — add a `normalize_terminal_output(&str) -> String`
   next to `replace_tabs` (`:246-256`) that ports `utils.ts:379-401` **whole**: the U+0E33 →
   `U+0E4D U+0E32` and U+0EB3 → `U+0ECD U+0EB2` decompositions first, then `\t` → three spaces.
   Document it against `utils.ts:368-402` and `tui.ts:1119-1128` the way the neighbouring helpers
   are documented.
2. **`crates/cyrup-tui/src/transcript/layout.rs`** — apply it inside `text_lines` (`:226-236`) to
   each `logical` before the `Span::styled` (pi's `text.ts:61` layer), so every `Text`-equivalent
   row is covered. Decide once whether `text_lines_of` needs it too (it takes an already-built
   `Line`, so the caller-side application in `text_lines` may be sufficient) and say so in the doc
   comment.
3. **`crates/cyrup-tui/src/transcript/tool_builtin.rs`** — route `render_generic` (`:409-426`, both
   the pretty-printed args loop and the result loop) and `render_extension` (`:390-407`, both the
   call-text and result-text loops) through the same helper. This is where ls/grep/find/bash and
   extension tabs actually reach the screen.
4. **`crates/cyrup-tui/src/text_width.rs`** — make `str_width` (`:20-22`) charge **3** columns per
   U+0009, matching `graphemeWidth` (`utils.ts:174-176`), so `wrap_line`
   (`transcript/layout.rs:43-60`) agrees with upstream on tab-bearing lines. No SARA AM width change:
   `unicode-width` already gives U+0E33 one column, which is pi's `+1` rule (`utils.ts:213-215`).

## Acceptance criteria

- [ ] A `normalize_terminal_output` (or equivalently-named) helper exists in
      `crates/cyrup-tui/src/transcript/layout.rs` and performs, in order, the two AM decompositions
      and the `\t` → `"   "` replacement.
- [ ] `grep -n "0E4D\|0ECD\|\\\\u{0e33}\|\\\\u{0eb3}" crates/cyrup-tui/src` returns hits (the guard
      exists at all).
- [ ] `text_lines` in `transcript/layout.rs` no longer passes an unnormalized `logical` into
      `Span::styled`.
- [ ] `render_generic` and `render_extension` in `transcript/tool_builtin.rs` contain no
      `Line::styled(l.to_string(), …)` over unnormalized output text.
- [ ] `crates/cyrup-tui/src/text_width.rs::str_width("\t") == 3` and `str_width("a\tb") == 5`.
- [ ] `crates/cyrup-tui/src/ansi.rs:42` still returns `true` for `0x09` — unchanged.
- [ ] `replace_tabs` and its three existing call sites (`tool_builtin.rs:47,97`, `layout.rs:276`)
      are still present and still behave identically; the read/write path is not re-plumbed.
- [ ] `cargo build -p cyrup-tui` → 0 warnings; `cargo clippy -p cyrup-tui --all-targets` → no new
      diagnostics.

## Constraints

- No tests are to be written for this task; another team owns the test suite.
- No benchmarks are to be written for this task.
- Workspace lints deny unwrap_used, expect_used, panic and indexing_slicing; cyrup-tui also has
  forbid(unsafe_code) and deny(clippy::string_slice).
- Mechanical fidelity to pi where pi's behaviour is the spec; Rust idiom where it is not.
