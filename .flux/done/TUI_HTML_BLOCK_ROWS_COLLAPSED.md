---
stage: done
status: completed
updated: 2026-08-28
---

# Keep A Multi-Line HTML Block On Separate Rendered Rows

> Identified by the `cyrup-tui` <-> pi port audit (fan-out + adversarial verification).
> **Priority:** low · **Effort:** small · Area: Markdown, latex, images, diffs and message rendering

## Objective

Raw HTML an assistant emits — a `<details>`/`<summary>` disclosure, an HTML table, hand-written
`<div>` markup — should print with its source line structure intact, as pi does. Today the whole
block collapses onto one long run-together line.

## Upstream reference

[`markdown.ts:612-617`](../../tmp/pi/packages/tui/src/components/markdown.ts) — the **block-level**
`case "html"` pushes `this.applyDefaultStyle(token.raw.trim())` as **one** entry that still contains
its newlines. `render()` then wraps every produced entry with `wrapTextWithAnsi(line, contentWidth)`
(`markdown.ts:322`), and `wrapTextWithAnsi` splits on `/\r\n|\r|\n/` (`utils.ts:839`) — so an N-line
`<details>…</details>` prints as N rows in source order.

`markdown.ts:721-724` is the separate **inline**-HTML arm, which concatenates into the surrounding
run. That behaviour is correct in cyrup today and must not change.

## Current state in cyrup-tui

- [`markdown/walk.rs:385-388`](../../crates/cyrup-tui/src/markdown/walk.rs) is a **single shared arm**
  for both:

  ```rust
  Event::Html(h) | Event::InlineHtml(h) => {
      let style = self.inline_style();
      self.push_text(h.trim_end_matches('\n'), style);
  }
  ```

- `push_text` ([`walk.rs:77-86`](../../crates/cyrup-tui/src/markdown/walk.rs)) appends to `self.cur`
  with no flush, and `flush_line` (`:191-217`) wraps through
  [`transcript::wrap_line`](../../crates/cyrup-tui/src/transcript/layout.rs) (`layout.rs:43-47`),
  which early-returns on width and **never splits on `'\n'`** — pi's `wrapTextWithAnsi` newline split
  has no counterpart here.
- `grep -n "HtmlBlock" crates/cyrup-tui/src/markdown/walk.rs` returns nothing: neither `start`
  (`:392`) nor `end` (`:492`) has an arm, so both fall through `_ => {}`.
- **The one-event-per-source-line assumption is verified against the vendored parser**, so no
  re-joining pass has to be undone: `pulldown-cmark-0.13.4/src/firstpass.rs:1241` and `:1290` call
  `append_html_line` once per line, `:1473-1499` appends one `ItemBody::Html` item each (two for
  CRLF), and `parse.rs:2261` maps each to its own `Event::Html` with no merging.
- There is no HTML coverage at all in `src/tests/markdown.rs`.

## Subtasks

1. **Split the shared arm** at
   [`markdown/walk.rs:385-388`](../../crates/cyrup-tui/src/markdown/walk.rs) so block-level
   `Event::Html` flushes per line and `Event::InlineHtml` keeps today's concatenating behaviour.
   Either give each event its own arm, or add `Tag::HtmlBlock` / `TagEnd::HtmlBlock` arms in `start`
   (`:392+`) and `end` (`:492+`) and let the flag select the behaviour — prefer whichever keeps the
   inline path a single unchanged expression.
2. Keep the existing `trim_end_matches('\n')` so a row does not gain a trailing empty span, and keep
   the surrounding blank-line/spacer behaviour of neighbouring blocks unchanged; the block should
   still route through the same `emit_prefixed` / prefix machinery so HTML nested inside a list item
   or blockquote keeps its indent.
3. Confirm the whole-block `trim` in pi (`token.raw.trim()`, `markdown.ts:615`) is respected at the
   block's edges — no leading or trailing empty row — while interior blank lines survive.

## Acceptance criteria

- [ ] `grep -n "Event::Html\|Event::InlineHtml" crates/cyrup-tui/src/markdown/walk.rs` shows the two
      events handled by different code paths
- [ ] A three-line `<details>\n<summary>x</summary>\n</details>` block renders as three transcript
      rows, in source order
- [ ] Inline HTML inside a paragraph (e.g. `a <b>bold</b> c`) still renders as one row, unchanged
- [ ] An HTML block nested in a `- ` list item keeps the item's continuation indent on rows 2..N
- [ ] No leading or trailing blank row is introduced around an HTML block
- [ ] `cargo build -p cyrup-tui --all-targets` — 0 errors, 0 warnings
- [ ] `cargo clippy -p cyrup-tui --all-targets` — warning count not increased
- [ ] `cargo test -p cyrup-tui` — no pre-existing test in `src/tests/markdown.rs` regresses

## Constraints

- Tests ARE in scope. (A prior revision of this file claimed "another team owns the test suite"; that was unfounded — `git log` over `crates/cyrup-tui/src/tests/` shows only the two authors already working here. It cost the alt-screen renderer its entire suite.)
- No benchmarks are to be written for this task.
- Workspace lints deny unwrap_used, expect_used, panic and indexing_slicing; cyrup-tui also has
  forbid(unsafe_code) and deny(clippy::string_slice).
- Mechanical fidelity to pi where pi's behaviour is the spec; Rust idiom where it is not.
