---
title: Promote the OSC-8 hyperlink regression tests into the tree
priority: LOW
tool: all
source: exec follow-up from the OSC-8 hyperlink task
stage: exec
status: done
updated: 2026-08-27 19:15
---

# The OSC-8 hyperlink feature shipped without permanent regression cover

## What happened

The OSC-8 task's brief closed with "Six files change. Nothing else." The executing
agent honoured that literally and added no permanent test module, verifying its
nine Definition-of-Done clauses with a temporary in-tree module that it then
deleted. That was the correct call under a no-scope-creep constraint, but it
leaves a feature with a non-trivial rendering contract and zero regression cover.

The default gate is `hyperlinks: false`
([`tool_render.rs:145`](../../../crates/cyrup-tui/src/transcript/tool_render.rs)),
so all 1274 existing cyrup-tui tests run the plain-text branch of
[`link_style`](../../../crates/cyrup-tui/src/transcript/tool_args.rs) (`:70-84`)
and would never reach the linked one.

---

## The implementation, as it actually is at HEAD

Read this section before writing a line; every assertion below is derived from it.

### `crates/cyrup-tui/src/osc.rs` — the whole mechanism

[`crates/cyrup-tui/src/osc.rs`](../../../crates/cyrup-tui/src/osc.rs)

| Symbol | Line | Contract |
| --- | --- | --- |
| `LINK_MASK` / `LINK_SHIFT` | `:57-58` | bits 9..=15 of `Modifier`, the seven ratatui leaves unallocated |
| `MAX_ID` | `:61` | `127` — the id space, and the number of links one paint can mark |
| `const _: () = assert!(Modifier::all().bits() & LINK_MASK == 0)` | `:65` | **compile-time** guard; a future ratatui allocating bit 9 breaks the build, not the screen. Nothing to test at runtime — a runtime restatement would need `LINK_MASK`, which is private, and would be strictly weaker than the `const` assert |
| `UNIT_WIDTH` | `:68-71` | `CellDiffOption::ForcedWidth(1)` |
| `LinkSink` | `:77-79` | `RefCell<Vec<String>>`, `pub(crate)` — reachable from `crate::transcript::tests` |
| `LinkSink::mark(url) -> Style` | `:88-97` | pushes `url`, returns `Modifier::from_bits_retain(id << 9)` with **`id = urls.len()`, one-based**; returns a neutral `Style` once `MAX_ID` ids are spent |
| `LinkSink::is_empty` | `:99-101` | `inject`'s early-out |
| `LinkSink::url_for(id)` | `:104-107` | `urls[id - 1]` — the exact inverse of `mark` |
| `open(url)` / `CLOSE` | `:112-117` | `"\x1b]8;;{url}\x07"` / `"\x1b]8;;\x07"`, BEL-terminated |
| `id_of(modifier)` | `:120-123` | `(bits & LINK_MASK) >> 9`, `0` ⇒ `None` |
| `inject(buf, sink)` | `:132-169` | walks `buf.content` linearly, finds each maximal run of one id, **clears the marker bits first** (`:154-156`, run resolved or not), then prepends `open(url)` to the run's head cell and appends `CLOSE` to its tail cell, stamping `UNIT_WIDTH` on both (`:160-167`). A one-cell run is `start == end` and ends up holding `open + symbol + CLOSE` |

The id is **pass-unique, not cyclic**, and `osc.rs:35-49` says why: a marked run
is not guaranteed to be contiguous, so `inject` must resolve *every* run of an id
to the same href.

### The wiring

* [`tool_args.rs:40-56`](../../../crates/cyrup-tui/src/transcript/tool_args.rs)
  `tool_path_span` — three arms. `StrArg::Invalid` → `"[invalid arg]"` in
  `error_style`, **unlinked**. `StrArg::Missing` → the `empty_fallback` (linked)
  or `"..."` in `tool_output_style` (**unlinked**). `StrArg::Value(p)` → linked.
* [`tool_args.rs:70-84`](../../../crates/cyrup-tui/src/transcript/tool_args.rs)
  `link_style` — `accent` unless BOTH `opts.hyperlinks` and `opts.links.is_some()`;
  href = `path_to_file_url(resolve_to_cwd(raw_path, opts.cwd ?? process cwd))`.
  Visible text is `shorten_path(raw)` — the **raw** path feeds the href, the
  **shortened** one feeds the span.
* [`tool_args.rs:90-96`](../../../crates/cyrup-tui/src/transcript/tool_args.rs)
  `push_search_path` — grep/find's `" in <path>"` tail. `tool_output_style`,
  never `link_style`. Deliberately inert.
* [`tool_args.rs:351`](../../../crates/cyrup-tui/src/transcript/tool_args.rs)
  `compact_read_call` — the collapsed `read resource …` / `[skill] …` header.
  `accent_style()`, never `link_style`. Deliberately inert.
* The **only four** linking call sites, all `tool_path_span`:
  [`tool_builtin.rs:21`](../../../crates/cyrup-tui/src/transcript/tool_builtin.rs)
  (`read`), `:71` (`write`), `:175` (`edit`), `:382` (`ls`, with `Some(".")` as
  the empty fallback).
* [`tool_render.rs:96-133`](../../../crates/cyrup-tui/src/transcript/tool_render.rs)
  `ImageOpts` — `cwd` `:108`, `hyperlinks` `:113`, `links` `:116`; `Default`
  `:135-152` seeds `hyperlinks: false` `:145`, `links: None` `:146`.
* [`cache.rs:28-56`](../../../crates/cyrup-tui/src/transcript/cache.rs)
  `cached_render` builds a fresh `LinkSink` (`:35`) and stores it **with** the
  lines, because the ids baked into the spans index that table
  ([`mod.rs:215-218`](../../../crates/cyrup-tui/src/transcript/mod.rs)).
* [`cache.rs:195-216`](../../../crates/cyrup-tui/src/transcript/cache.rs)
  `impl Component for TranscriptView::render` — renders the `Paragraph`, **then**
  `crate::osc::inject(frame.buffer_mut(), &self.render_cache.links)` at `:215`.
* [`draw.rs:146/161/191`](../../../crates/cyrup-tui/src/app/draw.rs) — the same
  three steps on the `insert_before` scrollback flush.
* [`view.rs:69`](../../../crates/cyrup-tui/src/transcript/view.rs)
  `set_hyperlinks` (bumps the render generation), `:76` `hyperlinks()`,
  `:153` `set_cwd`, `:159` `cwd()`.
* [`crates/cyrup-tools/src/path.rs:276-299`](../../../crates/cyrup-tools/src/path.rs)
  `path_to_file_url` — `SAFE = b"-._~!$&'()*+,;=:@/"`; everything else is
  `%`-escaped byte-wise, so `café` → `caf%C3%A9`, ` ` → `%20`, `#` → `%23`,
  `%` → `%25`. `:330-353` `resolve_to_cwd`.

### Why a marked run is not contiguous

[`layout.rs:139-174`](../../../crates/cyrup-tui/src/transcript/layout.rs)
`box_lines` runs [`wrap_line`](../../../crates/cyrup-tui/src/transcript/layout.rs)
(`:43-116`) at `content_width = width - padding_x * 2`, **left-pads** each
produced row with `Span::raw(" ")` (`:154-156`) and then
[`apply_bg`](../../../crates/cyrup-tui/src/transcript/layout.rs) (`:183-190`)
**right-pads** it to `width`. `wrap_line` carries the span style — marker bits
included — per grapheme across the break (`:98-113`). So a path long enough to
hard-break arrives as **two marked runs of the same id**, with unmarked padding
cells on both sides of the row boundary. That padding is what keeps `inject`'s
linear scan from fusing them, and it is why an id must name a link outright.

---

## The trap: `App::scrollback_text()` cannot see the escapes

[`shell.rs:288-290`](../../../crates/cyrup-tui/src/app/shell.rs) reads
`state.scrollback`, which [`draw.rs:174`](../../../crates/cyrup-tui/src/app/draw.rs)
fills from `lines` **before** `insert_before` runs `inject` at `:191`. The
accumulator therefore holds pre-injection `Line`s and contains **no escapes ever**.

Do not reach for it. The only place the OSC-8 bytes exist is a rendered `Buffer`,
so the harness must be `TranscriptView` → `Component::render` → `TestBackend`.

---

## Parity action — required path

Add exactly one file,
`crates/cyrup-tui/src/transcript/tests/osc_hyperlinks.rs`, and register it as
the **first** entry (the list is alphabetical) in
[`transcript/tests/mod.rs`](../../../crates/cyrup-tui/src/transcript/tests/mod.rs):

```rust
mod osc_hyperlinks;
mod output_pad;
mod progressive_commit;
```

Match the conventions already in that directory — the `#![allow]` block and
`use crate::transcript::*;` of
[`x_group.rs:7-15`](../../../crates/cyrup-tui/src/transcript/tests/x_group.rs),
and the `TestBackend` cell-walk of
[`render_cache.rs:16-28`](../../../crates/cyrup-tui/src/transcript/tests/render_cache.rs).
Do not invent a new style.

### Module preamble and the two helpers every test uses

```rust
//! TUI-020's permanent regression cover: the OSC-8 hyperlink contract.
//!
//! The escapes exist ONLY in a rendered `Buffer` — `crate::osc::inject`
//! (`osc.rs:132-169`) runs after the widget, at `cache.rs:215` for the live
//! viewport and `app/draw.rs:191` for the scrollback flush. `App::scrollback_text`
//! is filled BEFORE that (`app/draw.rs:174`) and can never observe them, so every
//! test here paints through `Component::render` into a `TestBackend` and reads
//! `cell.symbol()` back.
//!
//! MIRROR: clause 8's `!bel.contains("8;;")` assertion lives in
//! `crate::tests::tool_result_sanitize::osc_sequences_do_not_survive_as_literal_text`
//! (`src/tests/tool_result_sanitize.rs:62-73`) and is NOT duplicated here; this
//! module covers the interaction that file cannot see — a linked HEADER above a
//! result BODY whose own OSC-8 payload was stripped.
//!
//! KNOWN LIMITATION, deliberately untested (see "Out of scope" in the brief):
//! `inject` stamps `CellDiffOption::ForcedWidth(1)` (`osc.rs:68-71`) on the head
//! and tail cells of every run. When that cell holds a WIDE grapheme — a CJK path
//! component at a wrap boundary — the forced width understates the true column
//! count by one, so `Buffer::diff_iter` (ratatui-core-0.1.2
//! `buffer/diff.rs:133-140`) does not skip the grapheme's trailing continuation
//! cell and that row's diff accounting is off by one column. The fix is to capture
//! `cell.symbol().cell_width()` (trait `ratatui::buffer::CellWidth`,
//! `buffer/cell_width.rs:19-24`) BEFORE prepending the escape — `Cell::cell_width`
//! returns the forced value once it is set (`buffer/cell.rs:309-317`) — and that
//! is a change to `osc.rs`, which this task may not make.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use crate::ansi::strip_ansi;
use crate::transcript::*;
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use serde_json::json;

/// A deterministic session cwd. Absolute and outside `$HOME`, so `shorten_path`
/// (`tool_result.rs:261-269`) is the identity and no assertion depends on the
/// developer's environment.
const CWD: &str = "/tmp/aug-osc";

/// `\x1b]8;;<url>\x07` — `osc::open`, restated here because it is private.
fn open(url: &str) -> String {
    format!("\u{1b}]8;;{url}\u{7}")
}
/// `\x1b]8;;\x07` — `osc::CLOSE`.
const CLOSE: &str = "\u{1b}]8;;\u{7}";

/// One live tool run in a view whose gate and cwd are pinned.
fn view(tool: &str, args: serde_json::Value, hyperlinks: bool) -> TranscriptView {
    let mut v = TranscriptView::new();
    v.set_cwd(Some(std::path::PathBuf::from(CWD)));
    v.set_hyperlinks(hyperlinks);
    v.push_tool_start(tool, args);
    v
}

/// Paint the active region and concatenate every cell symbol, row by row — the
/// injected escapes ride in `cell.symbol()`, so this string is the real byte
/// stream the backend would print. Height must exceed the block's logical line
/// count or `render` tail-anchors and scrolls the head off (`cache.rs:200-206`).
fn paint(v: &mut TranscriptView, theme: &UiTheme, w: u16, h: u16) -> String {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|frame| {
        let area = frame.area();
        v.render(frame, area, theme);
    })
    .unwrap();
    let buf = term.backend().buffer();
    let area = buf.area;
    let mut out = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            if let Some(c) = buf.cell((x, y)) {
                out.push_str(c.symbol());
            }
        }
        out.push('\n');
    }
    out
}
```

Geometry that the assertions depend on, so do not change it casually. A collapsed
tool block is `1` spacer + `1` top pad + `N` header rows + `1` bottom pad
([`tool_render.rs:21-95`](../../../crates/cyrup-tui/src/transcript/tool_render.rs) →
`finalize_block` → `box_lines(_, width, 1, 1, _)`). `h = 12` clears every case
below. `padding_x = 1`, so the wrap width is `width - 2`.

Avoid `README.md`, `AGENTS.md`, `CLAUDE.md` and `SKILL.md` as file names in any
test whose header must be the plain `read <path>` form — those route through
`compact_read_classification`
([`tool_args.rs:286`](../../../crates/cyrup-tui/src/transcript/tool_args.rs))
into the unlinked compact header instead.

---

## The nine tests

### 1. `a_read_header_carries_the_open_and_close_escapes`

Pins: the escape reaches the `Buffer` at all, in BEL-terminated OSC-8 form, with
the href built from the resolved absolute path.

```rust
#[test]
fn a_read_header_carries_the_open_and_close_escapes() {
    let theme = UiTheme::dark();
    let mut v = view("read", json!({ "file_path": "/tmp/aug-osc/main.rs" }), true);
    let text = paint(&mut v, &theme, 60, 12);
    // One contiguous run at width 60 (content 58, header 25 cols): `inject` prepends
    // `open` to the head cell and appends CLOSE to the tail cell, so the row reads
    // back as one uninterrupted sequence.
    let href = "file:///tmp/aug-osc/main.rs";
    assert!(
        text.contains(&format!("{}{}{CLOSE}", open(href), "/tmp/aug-osc/main.rs")),
        "no OSC-8-wrapped path in:\n{text:?}"
    );
}
```

### 2. `the_href_is_the_raw_path_percent_encoded_and_the_text_is_shortened`

Pins: `path_to_file_url` encoding, and the `link_style` split where the href
takes the **raw** path and the span takes the **shortened** one.

Two halves, one test. The encoding half is environment-free; the `~` half reads
`$HOME` rather than writing it — mutating process env would race the six sibling
test binaries.

```rust
#[test]
fn the_href_is_the_raw_path_percent_encoded_and_the_text_is_shortened() {
    use cyrup_tools::path::path_to_file_url;
    use std::path::Path;

    // `path.rs:276-299` — SAFE excludes space, `#`, `%` and every non-ASCII byte.
    assert_eq!(path_to_file_url(Path::new("/tmp/café")), "file:///tmp/caf%C3%A9");
    assert_eq!(path_to_file_url(Path::new("/tmp/a b")), "file:///tmp/a%20b");
    assert_eq!(path_to_file_url(Path::new("/tmp/a#b")), "file:///tmp/a%23b");
    assert_eq!(path_to_file_url(Path::new("/tmp/a%b")), "file:///tmp/a%25b");

    // And through the render: `shorten_path` shortens the SPAN, never the href.
    let Ok(home) = std::env::var("HOME") else { return };
    if home.is_empty() || !home.starts_with('/') {
        return;
    }
    let raw = format!("{home}/aug osc/café.rs");
    let href = path_to_file_url(Path::new(&raw));
    assert!(href.contains("/aug%20osc/caf%C3%A9.rs"), "href not encoded: {href}");

    let theme = UiTheme::dark();
    let mut v = view("read", json!({ "file_path": raw.clone() }), true);
    let text = paint(&mut v, &theme, 70, 12);
    assert!(
        text.contains(&format!("{}~/aug osc/café.rs{CLOSE}", open(&href))),
        "the href must be raw+encoded and the text `~`-shortened:\n{text:?}"
    );
    let plain = strip_ansi(&text);
    assert!(plain.contains("~/aug osc/café.rs"), "visible text lost:\n{plain}");
    assert!(!plain.contains("%20"), "the encoded form must never be visible:\n{plain}");
}
```

### 3. `ls_links_the_session_cwd_and_the_two_unlinked_arms_emit_no_escape`

Pins all three arms of `tool_path_span` (`tool_args.rs:47-55`) in one test: the
`Some(".")` empty fallback that only `ls` passes, `[invalid arg]`, and `...`.

```rust
#[test]
fn ls_links_the_session_cwd_and_the_two_unlinked_arms_emit_no_escape() {
    let theme = UiTheme::dark();

    // `ls` with no `path` → `empty_fallback = Some(".")` → shorten_path(".") == "."
    // linked to `resolve_to_cwd(".", cwd)` == the session cwd.
    let mut ls = view("ls", json!({}), true);
    let text = paint(&mut ls, &theme, 60, 12);
    assert!(
        text.contains(&format!("{}.{CLOSE}", open("file:///tmp/aug-osc"))),
        "`ls` must link the session cwd:\n{text:?}"
    );

    // A non-string path → `StrArg::Invalid` → `[invalid arg]`, error_style, no link.
    let mut invalid = view("read", json!({ "file_path": 42 }), true);
    let text = paint(&mut invalid, &theme, 60, 12);
    assert!(strip_ansi(&text).contains("[invalid arg]"), "arm lost:\n{text:?}");
    assert!(!text.contains('\u{1b}'), "`[invalid arg]` must stay inert:\n{text:?}");

    // No path at all and no fallback → the `...` placeholder, tool_output_style, no link.
    let mut missing = view("read", json!({}), true);
    let text = paint(&mut missing, &theme, 60, 12);
    assert!(strip_ansi(&text).contains("read ..."), "arm lost:\n{text:?}");
    assert!(!text.contains('\u{1b}'), "the `...` placeholder must stay inert:\n{text:?}");
}
```

Note the second and third cases exercise `inject`'s `is_empty()` early-out
(`osc.rs:133-135`) as well: nothing was marked, so nothing is scanned.

### 4. `grep_find_tails_and_the_compact_read_header_stay_unlinked`

Pins the deliberate non-parity: only four call sites link, and `push_search_path`
/ `compact_read_call` are not among them. Each half asserts the text is still
**present**, so the test cannot pass by the tail simply disappearing.

```rust
#[test]
fn grep_find_tails_and_the_compact_read_header_stay_unlinked() {
    let theme = UiTheme::dark();

    for tool in ["grep", "find"] {
        let mut v = view(tool, json!({ "pattern": "x", "path": "/tmp/aug-osc" }), true);
        let text = paint(&mut v, &theme, 60, 12);
        assert!(
            strip_ansi(&text).contains("/tmp/aug-osc"),
            "`{tool}` lost its ` in <path>` tail:\n{text:?}"
        );
        assert!(
            !text.contains('\u{1b}'),
            "`push_search_path` is deliberately unlinked (`tool_args.rs:90-96`):\n{text:?}"
        );
    }

    // The collapsed compact `read` header is `compact_read_call` (`tool_args.rs:351`),
    // which never reaches `tool_path_span`.
    let mut v = view("read", json!({ "file_path": "/tmp/aug-osc/CLAUDE.md" }), true);
    let text = paint(&mut v, &theme, 60, 12);
    assert!(
        strip_ansi(&text).contains("read resource CLAUDE.md"),
        "compact header lost:\n{text:?}"
    );
    assert!(!text.contains('\u{1b}'), "the compact header is unlinked:\n{text:?}");
}
```

### 5. `the_gate_off_buffer_is_byte_identical_to_today`

Pins pi's `if (!getCapabilities().hyperlinks) return styledText` early return
(`tool_args.rs:71-73`) — the branch every one of the other 1274 tests runs.

```rust
#[test]
fn the_gate_off_buffer_is_byte_identical_to_today() {
    let theme = UiTheme::dark();
    let args = json!({ "file_path": "/tmp/aug-osc/main.rs" });
    let mut off = view("read", args.clone(), false);
    let text = paint(&mut off, &theme, 60, 12);
    assert!(text.contains("/tmp/aug-osc/main.rs"), "path lost:\n{text:?}");
    assert!(!text.contains('\u{1b}'), "no ESC with the gate off:\n{text:?}");
    assert!(!text.contains("]8;;"), "no OSC-8 payload with the gate off:\n{text:?}");

    // And the gate-on render is the same STRING once the escapes are stripped —
    // the same buffer, plus escapes, never plus columns.
    let mut on = view("read", args, true);
    assert_eq!(strip_ansi(&paint(&mut on, &theme, 60, 12)), text);
}
```

### 6. `a_wrapped_path_emits_one_pair_per_row_with_the_same_href`

**The clause the `osc.rs:35-49` design note exists for.** At width 40 the content
width is 38; the path token below is 64 columns, so `wrap_line`'s `breakLongWord`
arm (`layout.rs:72-86`) hard-breaks it into a 38-column piece and a 26-column
piece on two rows, each left-padded and right-padded by `box_lines`/`apply_bg`.
That is two marked runs of one id.

```rust
#[test]
fn a_wrapped_path_emits_one_pair_per_row_with_the_same_href() {
    let theme = UiTheme::dark();
    let path = "/tmp/aug-osc/a-really-long-directory-name/and-another-one/file.rs";
    let href = format!("file://{path}");

    let mut on = view("read", json!({ "file_path": path }), true);
    let linked = paint(&mut on, &theme, 40, 12);
    let mut off = view("read", json!({ "file_path": path }), false);
    let plain = paint(&mut off, &theme, 40, 12);

    // Columns do not move: the escapes are the ONLY difference between the buffers.
    assert_eq!(strip_ansi(&linked), plain, "the wrap moved a column");

    // One open/close pair PER ROW — `box_lines` padding breaks the run in two, and
    // `inject` resolves both to the SAME href because the id names the link outright.
    assert_eq!(
        linked.matches(&open(&href)).count(),
        2,
        "expected one `open` per wrapped row:\n{linked:?}"
    );
    assert_eq!(linked.matches(CLOSE).count(), 2, "unbalanced close:\n{linked:?}");
    // No OTHER href was emitted — a cyclic id scheme would have produced one.
    assert_eq!(linked.matches("\u{1b}]8;;file://").count(), 2, "stray href:\n{linked:?}");
}
```

### 7. `two_links_in_one_pass_resolve_to_distinct_hrefs`

**The most important test in the module.** This is the regression the brief's own
original global `seen` counter would have shipped, and the reason `mark`
(`osc.rs:88-97`) hands out pass-unique one-based ids instead. Both runs are live
in the same `active_tools` list, so both are marked against the one `LinkSink`
`cached_render` built at `cache.rs:35`.

```rust
#[test]
fn two_links_in_one_pass_resolve_to_distinct_hrefs() {
    let theme = UiTheme::dark();
    let mut on = TranscriptView::new();
    on.set_cwd(Some(std::path::PathBuf::from(CWD)));
    on.set_hyperlinks(true);
    on.push_tool_start("read", json!({ "file_path": "/tmp/aug-osc/first.rs" }));
    on.push_tool_start("write", json!({ "file_path": "/tmp/aug-osc/second.rs" }));
    let linked = paint(&mut on, &theme, 60, 16);

    // Each visible path is wrapped by ITS OWN href. A cyclic counter links the
    // second header to the first file — this pair of asserts is what catches it.
    for name in ["first.rs", "second.rs"] {
        let p = format!("/tmp/aug-osc/{name}");
        assert!(
            linked.contains(&format!("{}{p}{CLOSE}", open(&format!("file://{p}")))),
            "`{name}` is not wrapped by its own href:\n{linked:?}"
        );
    }
    assert_eq!(linked.matches("\u{1b}]8;;file://").count(), 2, "id reuse:\n{linked:?}");

    // Columns do not move, and the content height is gate-independent.
    let mut off = TranscriptView::new();
    off.set_cwd(Some(std::path::PathBuf::from(CWD)));
    off.set_hyperlinks(false);
    off.push_tool_start("read", json!({ "file_path": "/tmp/aug-osc/first.rs" }));
    off.push_tool_start("write", json!({ "file_path": "/tmp/aug-osc/second.rs" }));
    assert_eq!(strip_ansi(&linked), paint(&mut off, &theme, 60, 16));
    assert_eq!(on.content_height(60, &theme), off.content_height(60, &theme));
}
```

### 8. `a_linked_header_sits_above_a_result_body_whose_own_osc_8_was_stripped`

Clause 8's original assertion — `!bel.contains("8;;")` — is **already permanently
covered** at
[`src/tests/tool_result_sanitize.rs:62-73`](../../../crates/cyrup-tui/src/tests/tool_result_sanitize.rs).
Do not duplicate it. Cover instead the interaction those two mechanisms have and
neither file currently sees: `result_text`
([`tool_result.rs:59-76`](../../../crates/cyrup-tui/src/transcript/tool_result.rs))
strips the BODY's escapes at materialisation time, while `inject` adds the
HEADER's after the widget has run — opposite directions, one buffer.

```rust
#[test]
fn a_linked_header_sits_above_a_result_body_whose_own_osc_8_was_stripped() {
    let theme = UiTheme::dark();
    let mut v = view("ls", json!({}), true);
    v.push_tool_end(
        "ls",
        false,
        Some(json!({ "content": [{ "type": "text",
            "text": "\u{1b}]8;;file:///tmp/evil\u{7}linked\u{1b}]8;;\u{7}\nplain.txt" }] })),
    );
    let text = paint(&mut v, &theme, 60, 16);

    // The header's escape survives — it was added AFTER the widget wrote the cells.
    assert!(
        text.contains(&format!("{}.{CLOSE}", open("file:///tmp/aug-osc"))),
        "header link lost:\n{text:?}"
    );
    // The body's did not — `result_text` → `ansi::sanitize_display_text` removed it.
    assert!(!text.contains("file:///tmp/evil"), "body OSC-8 survived:\n{text:?}");
    let plain = strip_ansi(&text);
    assert!(plain.contains("linked"), "body content lost:\n{plain}");
    assert!(plain.contains("plain.txt"), "body content lost:\n{plain}");
}
```

### 9. `there_is_no_visible_url_fallback`

Clause 9's second half — "`image.rs` and `ansi.rs` are untouched" — is a review
statement about a diff, not a runtime property; it is true at HEAD and is not
testable. What **is** testable, and is the half that matters, is the first half:
a linked header never grows a ` (url)` suffix. Pi's `linkPath`
(`render-utils.ts:19-23`) returns the styled text wrapped, never annotated.

```rust
#[test]
fn there_is_no_visible_url_fallback() {
    let theme = UiTheme::dark();
    for (tool, args) in [
        ("read", json!({ "file_path": "/tmp/aug-osc/main.rs" })),
        ("write", json!({ "file_path": "/tmp/aug-osc/main.rs" })),
        ("edit", json!({ "file_path": "/tmp/aug-osc/main.rs" })),
        ("ls", json!({ "path": "/tmp/aug-osc" })),
    ] {
        let mut on = view(tool, args.clone(), true);
        let linked = paint(&mut on, &theme, 60, 16);
        let mut off = view(tool, args, false);
        let plain = paint(&mut off, &theme, 60, 16);

        assert!(linked.contains("\u{1b}]8;;file://"), "`{tool}` must link:\n{linked:?}");
        let visible = strip_ansi(&linked);
        assert!(!visible.contains("(file://"), "`{tool}` grew a url suffix:\n{visible}");
        assert!(!visible.contains(" (url"), "`{tool}` grew a url suffix:\n{visible}");
        // The strongest form of the same claim: the gate adds bytes, never columns.
        assert_eq!(visible, plain, "`{tool}` moved a column when linked");
    }
}
```

This is also the test that covers the fourth linking call site, `edit`
([`tool_builtin.rs:175`](../../../crates/cyrup-tui/src/transcript/tool_builtin.rs)),
which no other test above reaches. `edit` with no `old_string`/`new_string`
renders the header and an empty diff; that is fine — the header is the subject.

---

## Out of scope — recorded, not tested

* **`ForcedWidth(1)` understates a wide head/tail grapheme.** `inject`
  ([`osc.rs:160-167`](../../../crates/cyrup-tui/src/osc.rs)) stamps
  `UNIT_WIDTH` (`:68-71`) unconditionally. That is exact for an ASCII head cell,
  which is what every path in practice starts with (`/` or `~`), but a path with
  a CJK component reaches the case: `read プロジェクト/x.rs` puts a two-column
  grapheme in the head cell, and a CJK component landing on a wrap boundary puts
  one at the head of a continuation row. `Buffer::diff_iter` advances by
  `cell.cell_width()` (ratatui-core-0.1.2 `buffer/diff.rs:133-140`), so with the
  width forced to `1` the grapheme's trailing continuation cell is not skipped
  and that row's diff column accounting is off by one.

  **Decision: no test here.** The fix is `let w = cell.symbol().cell_width();`
  captured **before** the escape is prepended — `Cell::cell_width` returns the
  forced value once set (`buffer/cell.rs:309-317`), so reading it after is
  useless — then `ForcedWidth(NonZeroU16::new(w.max(1)))`. That edits `osc.rs`,
  which this task's DoD forbids. A test written now would either assert the
  defect (and be deleted by the fix) or fail today; both are worse than the
  module-doc note prescribed above. Raise it as a separate follow-up,
  `LOW-osc-8-forcedwidth-understates-a-wide-head-grapheme`.

* **The `MAX_ID = 127` cap.** Exercising it needs 128 linked headers in one
  paint. The overflow behaviour is the pre-existing plain-text one
  (`osc.rs:90-92` returns a neutral `Style`), not a wrong href, so the cost of
  the test exceeds its value. Not one of the nine.

* **The `const _: () = assert!(...)` guard** (`osc.rs:65`) is a compile-time
  check over a private constant. It cannot be restated at runtime from the test
  module and would be weaker if it could. Already covered by every build.

---

## Definition of done

1. `crates/cyrup-tui/src/transcript/tests/osc_hyperlinks.rs` exists with the nine
   `#[test]` functions named above, in that order, each carrying the doc comment
   that names the clause it pins.
2. It is registered as the first `mod` line in
   `crates/cyrup-tui/src/transcript/tests/mod.rs`.
3. `cargo test -p cyrup-tui` is green; the count rises by nine and no existing
   test changes.
4. `cargo clippy --workspace --all-targets` is clean under the workspace's
   deny-by-default lints (hence the `#![allow]` header, which is exactly the one
   every sibling test module in that directory already carries).
5. **No production file changes.** The only files touched are the new test module
   and `transcript/tests/mod.rs`. In particular `osc.rs`, `tool_args.rs`,
   `tool_render.rs`, `cache.rs`, `app/draw.rs` and `cyrup-tools/src/path.rs` are
   read-only for this task.
