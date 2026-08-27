---
stage: todo
status: pending
updated: 2026-08-27
---

# Echo User Messages With Their Source List Markers And Backslash Escapes Intact

> Identified by the `cyrup-tui` <-> pi port audit (fan-out + adversarial verification).
> **Priority:** medium · **Effort:** small · Area: Markdown, latex, images, diffs and message rendering

## Objective

A user's own prompt should echo back in the transcript exactly as they typed it. Today it is
re-normalized by the general markdown renderer: `* a` / `* b` echoes as `- a` / `- b`, three
consecutive `1.` items echo renumbered `1.` / `2.` / `3.`, and `\*literal\*` (or a path with escaped
punctuation) loses its backslashes. Assistant and thinking bodies must keep today's normalizing
behaviour — this is a user-message-only fidelity rule upstream.

## Upstream reference

[`user-message.ts:50-54`](../../tmp/pi/packages/coding-agent/src/modes/interactive/components/user-message.ts)
is the **only** place in pi that sets `preserveOrderedListMarkers: true` and
`preserveBackslashEscapes: true`, deliberately, on the `Markdown` component wrapping the user's text.

[`markdown.ts:220-224`](../../tmp/pi/packages/tui/src/components/markdown.ts) documents both flags
("Preserve source list markers instead of normalizing them" / "Preserve source backslash escapes
instead of normalizing escaped punctuation").

- **Markers.** `getOrderedListMarker` / `getUnorderedListMarker` (`markdown.ts:743-751`) recover the
  literal marker from `item.raw` with `/^(?: {0,3})(\d{1,9}[.)])[ \t]+/` and
  `/^(?: {0,3})([-+*])(?:[ \t]+|(?=\r?\n|$))/`. `renderList` (`:765-771`) uses them in place of the
  synthesized `` `${startNumber + i}. ` `` / `"- "`. Note the **single** flag
  `preserveOrderedListMarkers` gates **both** branches — ordered and unordered — with `?? ` falling
  back to the synthesized marker when the regex does not match.
- **Escapes.** `markdown.ts:655-657`: `case "escape": result += applyTextWithNewlines(
  this.options.preserveBackslashEscapes ? token.raw : token.text)` — i.e. `\*` re-emits as `\*`
  rather than `*`.

## Current state in cyrup-tui

- [`markdown/walk.rs:412-423`](../../crates/cyrup-tui/src/markdown/walk.rs) — the `Tag::Item` arm
  synthesizes the marker unconditionally from the list's own counter: `format!("{n}. ")` (`:415`)
  or the literal `"- "` (`:419`). It never looks at the `raw` slice, **even though the slice is
  already threaded in**: `event(ev, raw)` at `:330-331` passes it to `start(tag, raw)`, and the
  `Tag::Strikethrough` arm at `:445+` uses exactly that mechanism to implement pi's strict `~~` rule.
  The plumbing this task needs is present and proven.
- [`markdown/walk.rs:334-341`](../../crates/cyrup-tui/src/markdown/walk.rs) — `Event::Text` emits
  pulldown-cmark's already-unescaped text. There is no escape-aware arm.
- [`markdown/mod.rs:192-247`](../../crates/cyrup-tui/src/markdown/mod.rs) — the renderer's option
  fields are `default_text`, `default_italic`, `hyperlinks`, `math`, `strike_literal`. There are no
  preserve flags, and the public entry points are `render` (`:99`), `render_with_text_color`
  (`:110`), `render_with_default_style` (`:132`) and `render_with_hyperlink_support` (`:155`).
- [`transcript/render.rs:25-31`](../../crates/cyrup-tui/src/transcript/render.rs) — `Entry::User`
  renders through `crate::markdown::render_with_text_color`, the same general-purpose entry point the
  assistant thinking body uses, with no per-message options.

No comment anywhere in cyrup-tui records this as a deliberate divergence.

## Subtasks

1. **Add the two option fields** to the renderer state in
   [`markdown/mod.rs`](../../crates/cyrup-tui/src/markdown/mod.rs), defaulted off so every existing
   entry point keeps today's behaviour byte-for-byte.
2. **Marker recovery** in the `Tag::Item` arm of
   [`markdown/walk.rs:412-423`](../../crates/cyrup-tui/src/markdown/walk.rs): when the flag is on,
   parse the literal marker off the front of the item's `raw` slice using pi's two patterns
   (`markdown.ts:743-751`), and fall back to the synthesized marker when neither matches. The
   ordered counter must still advance so a later item that *does* fall back is numbered correctly.
3. **Escape re-emission**: when the flag is on, an escaped punctuation character must reach the
   output as its source `\x` (pi `markdown.ts:656`). pulldown-cmark does not emit a distinct escape
   event, so recover it from the `raw` slice of the text run the same way the strikethrough arm
   recovers its delimiters.
4. **A user-message-only entry point** in [`markdown/mod.rs`](../../crates/cyrup-tui/src/markdown/mod.rs)
   that turns both flags on (it needs the text colour too, so it is a variant of
   `render_with_text_color`), called from
   [`transcript/render.rs:25`](../../crates/cyrup-tui/src/transcript/render.rs).

## Acceptance criteria

- [ ] `grep -n "preserve" crates/cyrup-tui/src/markdown/mod.rs` shows two new option fields, both
      defaulting to off
- [ ] The `Tag::Item` arm in `markdown/walk.rs` reads the `raw` slice when the flag is set;
      `grep -n 'raw' crates/cyrup-tui/src/markdown/walk.rs` shows the item arm among the consumers
- [ ] A user message typed `* a\n* b` echoes with `*` bullets; typed `+ a` echoes with `+`; typed
      `1)` echoes with `1)`
- [ ] Three consecutive `1.` items in a user message echo as `1.` / `1.` / `1.`, not renumbered
- [ ] `\*literal\*` in a user message echoes with its backslashes; the same text in an **assistant**
      message still echoes as `*literal*`
- [ ] A `- ` bullet in an **assistant** message still renders as `- ` (the four existing public
      entry points are unchanged in behaviour)
- [ ] Only [`transcript/render.rs`](../../crates/cyrup-tui/src/transcript/render.rs)'s `Entry::User`
      arm calls the new entry point — `grep -rn "<new fn name>" crates/cyrup-tui/src` returns exactly
      the definition and that one call site
- [ ] `cargo build -p cyrup-tui --all-targets` — 0 errors, 0 warnings
- [ ] `cargo clippy -p cyrup-tui --all-targets` — warning count not increased
- [ ] `cargo test -p cyrup-tui` — no pre-existing test in `src/tests/markdown.rs` regresses

## Constraints

- No tests are to be written for this task; another team owns the test suite.
- No benchmarks are to be written for this task.
- Workspace lints deny unwrap_used, expect_used, panic and indexing_slicing; cyrup-tui also has
  forbid(unsafe_code) and deny(clippy::string_slice).
- Mechanical fidelity to pi where pi's behaviour is the spec; Rust idiom where it is not.
