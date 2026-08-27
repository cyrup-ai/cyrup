---
title: Promote the OSC-8 hyperlink regression tests into the tree
priority: LOW
tool: all
source: exec follow-up from the OSC-8 hyperlink task
stage: aug
status: in-progress
updated: 2026-08-27 14:35
---

# The OSC-8 hyperlink feature shipped without permanent regression cover

## What happened

The OSC-8 task's brief closed with "Six files change. Nothing else." The executing
agent honoured that literally and added no permanent test module, verifying its
nine Definition-of-Done clauses with a temporary in-tree module that it then
deleted. That was the correct call under a no-scope-creep constraint, but it
leaves a feature with a non-trivial rendering contract and zero regression cover.

The default gate is `hyperlinks: false`, so the existing 1272 cyrup-tui tests all
run the plain-text branch and would not catch a regression on the linked branch.

## What the temporary tests covered

Worth reconstructing rather than reinventing — these are the nine behaviours that
were demonstrated green and then discarded:

1. A `read` header with the gate on contains `\x1b]8;;file:///…\x07` and the
   closing `\x1b]8;;\x07`.
2. `path_to_file_url` round-trips spaces, `#`, `%` and non-ASCII (`/tmp/café` →
   `file:///tmp/caf%C3%A9`); visible text stays `~`-shortened.
3. `ls` with no `path` links the session cwd; `[invalid arg]` and the `...`
   placeholder emit no ESC at all.
4. `grep`/`find` tails stay inert — `push_search_path` and `compact_read_call` are
   deliberately unlinked, matching pi, where only four callers link.
5. With the gate off the buffer is byte-identical: no ESC, no `]8;;`, no ` (url)`.
6. At a width that forces `box_lines` to wrap a long path, `strip_ansi` of the
   linked render equals the plain render byte-for-byte.
7. Columns do not move: `plain == strip_ansi(linked)` and `content_height` is
   equal with and without the gate. Two links in one pass resolve to distinct
   hrefs — the case the brief's original global `seen` counter got wrong.
8. `tool_result_sanitize`'s `!bel.contains("8;;")` assertion still holds.
9. No ` (url)` fallback exists anywhere; `image.rs` and `ansi.rs` are untouched.

## Parity action

Add `crates/cyrup-tui/src/transcript/tests/osc_hyperlinks.rs` covering the nine
behaviours above, driving `TranscriptView` through `Component::render` into a
`TestBackend`, and register it in `transcript/tests/mod.rs`.

Clause 7 is the one that matters most: it is the regression that the brief's own
prescribed design would have shipped.

## Definition of done

1. The nine behaviours are asserted by a permanent test module.
2. The suite is green with the gate both on and off.
3. No production file changes.
