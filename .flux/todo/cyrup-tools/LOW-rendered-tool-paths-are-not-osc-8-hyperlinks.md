---
title: Rendered tool paths are not OSC-8 hyperlinks
priority: LOW
tool: all
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: exec
status: done
updated: 2026-08-27
---

# Rendered tool paths are not OSC-8 hyperlinks

## Core objective

Make the file path in a `read` / `write` / `edit` / `ls` tool header **clickable** in a terminal
that forwards OSC-8, exactly as pi's `linkPath` does, and leave it byte-identical to today on every
terminal that does not.

**The change lands in `crates/cyrup-tui`, not `crates/cyrup-tools`.** This task file sits under
`.flux/todo/cyrup-tools/` and its frontmatter says `tool: all`; both are misfiled. The renderer that
emits the header is [`transcript/tool_args.rs`](../../../crates/cyrup-tui/src/transcript/tool_args.rs),
the capability gate is [`image.rs`](../../../crates/cyrup-tui/src/image.rs), and the paint seam is
[`app/draw.rs`](../../../crates/cyrup-tui/src/app/draw.rs) +
[`transcript/cache.rs`](../../../crates/cyrup-tui/src/transcript/cache.rs). One small addition —
a `path → file:// URL` encoder — belongs in `crates/cyrup-tools` because the *inverse* already lives
there.

## What pi does — verified

[`render-utils.ts`](../../../tmp/pi/packages/coding-agent/src/core/tools/render-utils.ts), lines
10-23 and 75-85, verbatim:

```ts
export function shortenPath(path: unknown): string {
	if (typeof path !== "string") return "";
	const home = os.homedir();
	if (path.startsWith(home)) {
		return `~${path.slice(home.length)}`;
	}
	return path;
}

export function linkPath(styledText: string, rawPath: string, cwd: string): string {
	if (!getCapabilities().hyperlinks) return styledText;
	const absolutePath = resolvePath(rawPath, cwd);
	return hyperlink(styledText, pathToFileURL(absolutePath).href);
}

export function renderToolPath(
	rawPath: string | null,
	theme: Theme,
	cwd: string,
	options?: { emptyFallback?: string },
): string {
	if (rawPath === null) return invalidArgText(theme);
	const value = rawPath || options?.emptyFallback;
	if (!value) return theme.fg("toolOutput", "...");
	return linkPath(theme.fg("accent", shortenPath(value)), value, cwd);
}
```

Three properties that the implementation must honour, all readable off those lines:

1. The link wraps the **already-styled, `~`-shortened** display text; the href is built from the
   **unshortened raw** path resolved against the session cwd. Shortening is display-only.
2. Only the `accent` arm is linked. `[invalid arg]` (`invalidArgText`, `:71-73`) and the
   `toolOutput`-coloured `...` (`:83`) are **not** links. The `emptyFallback` arm **is** — `value`
   is `rawPath || options?.emptyFallback`, and `linkPath` receives that same `value`.
3. The gate is `getCapabilities().hyperlinks`; when false the function returns the styled text
   unchanged, with no escape and no ` (url)` consolation suffix.

### Correction to the original claim

> "Every built-in tool header path goes through this."

**False, and the correction constrains the work.** `renderToolPath` has exactly four callers —
verified with `grep -rn "renderToolPath" tmp/pi/packages/coding-agent/src`:

| caller | line | call |
| --- | --- | --- |
| [read.ts](../../../tmp/pi/packages/coding-agent/src/core/tools/read.ts) | `:81` | `renderToolPath(str(args?.file_path ?? args?.path), theme, cwd)` |
| [write.ts](../../../tmp/pi/packages/coding-agent/src/core/tools/write.ts) | `:146` | `renderToolPath(rawPath, theme, cwd)` |
| [edit.ts](../../../tmp/pi/packages/coding-agent/src/core/tools/edit.ts) | `:225` | `renderToolPath(str(args?.file_path ?? args?.path), theme, cwd)` |
| [ls.ts](../../../tmp/pi/packages/coding-agent/src/core/tools/ls.ts) | `:59` | `renderToolPath(str(args?.path), theme, cwd, { emptyFallback: "." })` |

`grep` and `find` do **not** link their `" in <path>"` tail. Both import bare `shortenPath` and
build the tail themselves — [grep.ts:79](../../../tmp/pi/packages/coding-agent/src/core/tools/grep.ts)
and [find.ts:76](../../../tmp/pi/packages/coding-agent/src/core/tools/find.ts) are both
`const path = rawPath !== null ? shortenPath(rawPath || ".") : null;`. So cyrup's
[`push_search_path`](../../../crates/cyrup-tui/src/transcript/tool_args.rs) (tool_args.rs:45-55)
is **already correct and must not be touched.** Linking it would be the "behaviour pi does not
have" this task forbids.

The compact `read` header ([`compact_read_call`](../../../crates/cyrup-tui/src/transcript/tool_args.rs),
tool_args.rs:207-229) is likewise **not** linked upstream — `formatCompactReadCall`
(`read.ts:145-167`) emits `theme.fg("accent", classification.label)` with no `linkPath`. Leave it.

## What cyrup does today — verified

[tool_args.rs:26-43](../../../crates/cyrup-tui/src/transcript/tool_args.rs):

```rust
/// `renderToolPath` (render-utils.ts:75-85): `[invalid arg]` for a non-string, the `emptyFallback`
/// (else `...`) for an empty/absent path, otherwise the `~`-shortened path in accent. Hyperlinks are a
/// terminal escape the cell grid does not carry (tracked residual).
pub(super) fn tool_path_span(
    args: &Value,
    keys: &[&str],
    empty_fallback: Option<&str>,
    theme: &UiTheme,
) -> Span<'static> {
    match str_arg(args, keys) {
        StrArg::Invalid => Span::styled("[invalid arg]".to_string(), theme.error_style()),
        StrArg::Missing => match empty_fallback {
            Some(f) => Span::styled(shorten_path(f), theme.accent_style()),
            None => Span::styled("...".to_string(), theme.tool_output_style()),
        },
        StrArg::Value(p) => Span::styled(shorten_path(&p), theme.accent_style()),
    }
}
```

Four call sites, matching pi's four exactly — verified in
[tool_builtin.rs](../../../crates/cyrup-tui/src/transcript/tool_builtin.rs) at `:21` (`read`),
`:71` (`write`), `:170` (`edit`) and `:374` (`ls`, with `Some(".")`).

The crate emits **zero** OSC-8 anywhere. The original claim's "the crate does implement OSC-8
elsewhere" is false and its own refutation section already says so; confirmed again here:
[markdown/mod.rs:142-161](../../../crates/cyrup-tui/src/markdown/mod.rs) and
[markdown/walk.rs:558-579](../../../crates/cyrup-tui/src/markdown/walk.rs) only consult
`hyperlinks` to decide whether to append the legacy ` (url)` suffix, and
[login_dialog.rs:41-47](../../../crates/cyrup-tui/src/login_dialog.rs) records the same
crate-wide omission and names `TUI-020`. **This task is TUI-020's first landing.**

Note that [app/state.rs:152](../../../crates/cyrup-tui/src/app/state.rs) already documents the
module this work creates — *"The `hyperlinks` flag gates OSC-8 emission in rendered links
(`osc::hyperlink`)"* — for a `crate::osc` that does not yet exist. Create it at that name.

## The two real obstacles, resolved

### 1. The sanitizer is NOT an obstacle

[`ansi::strip_ansi`](../../../crates/cyrup-tui/src/ansi.rs) and `sanitize_display_text` run over
**tool-result text only**. Every call site is in
[tool_result.rs](../../../crates/cyrup-tui/src/transcript/tool_result.rs) — `:61`, `:68`, `:90`,
`:94`, `:99`, all inside `result_text` — plus [bash.rs:138](../../../crates/cyrup-tui/src/bash.rs).
**No header span is ever passed through it.** A cyrup-authored OSC-8 sequence on a tool header can
therefore never be stripped by cyrup's own sanitizer.

The existing sanitizer tests stay green for the same reason plus a second one: they assert on
`app.scrollback_text()`, which reads `AppState::scrollback` — a `Vec<Line>` accumulated in
[draw.rs:166-167](../../../crates/cyrup-tui/src/app/draw.rs) **before** the buffer render. The
escape prescribed below is injected into `Buffer` **cells**, downstream of that clone, so
`scrollback_text()` never sees it and
[tests/tool_result_sanitize.rs:63-72](../../../crates/cyrup-tui/src/tests/tool_result_sanitize.rs)
— which asserts `!bel.contains("8;;")` on an `ls` **result** — remains true and remains meaningful.

### 2. The `Span` is the obstacle. The `Cell` is the answer.

An escape cannot live in a `Span`. `Span::styled_graphemes`
(ratatui-core-0.1.2 `src/text/span.rs:311-317`) is
`.filter(|g| !g.contains(char::is_control))`, so the `ESC` is deleted on the way into the buffer and
`]8;;file:///…` survives as literal visible text — the exact failure
[ansi.rs:13-18](../../../crates/cyrup-tui/src/ansi.rs) describes. `Span::width` /
[`text_width::str_width`](../../../crates/cyrup-tui/src/text_width.rs) would also count every one of
those bytes as a column, corrupting `wrap_line`, `wrapped_height` and the inline viewport height.

ratatui 0.30 has a first-class mechanism for exactly this, and `ratatui-image` — already a
dependency — uses it. `ratatui-core-0.1.2/src/buffer/cell.rs:22-32`:

```rust
    /// Force a width regardless of the symbol text width.
    ///
    /// Escape sequences will have some computed width that does match what is written to the
    /// screen.
    ForcedWidth(NonZeroU16),
```

`Cell::set_symbol` (`cell.rs:155-159`) stores the string **unfiltered**, and the crossterm backend
prints it verbatim — `queue!(self.writer, Print(cell.symbol()))`
(`ratatui-crossterm-0.1.2/src/lib.rs:272`). `ratatui-image` builds its Kitty/iTerm2/sixel output
this way (`protocol.rs:31` `const UNIT_WIDTH: CellDiffOption = CellDiffOption::ForcedWidth(NonZeroU16::new(1).unwrap());`,
applied at `iterm2.rs:83`, `kitty.rs:213`, `sixel.rs:87`).

`ForcedWidth` is **mandatory**, not optional. This crate builds with
`default = ["wasm-host", "scrolling-regions"]`
([cyrup-tui/Cargo.toml:24](../../../crates/cyrup-tui/Cargo.toml)), so `insert_before` takes
`insert_before_scrolling_regions` → `draw_lines_over_cleared` → `self.backend.draw(old.diff_iter(&new))`.
`diff_iter` advances by `cell_width()` (`ratatui-core-0.1.2/src/buffer/diff.rs:132-142`); without
the override, a cell holding `\x1b]8;;file:///home/u/x.rs\x07~` measures ~25 columns and the
iterator would skip 24 real cells. With `ForcedWidth(1)` it advances by one and emits the cell.

`CellDiffOption::Skip` must **not** be used — `diff_iter` drops skipped cells entirely
(`diff.rs:128-129`), which would delete the escape.

**Do not touch [`image.rs:341-350`](../../../crates/cyrup-tui/src/image.rs)'s half-block decision.**
That comment is about *tool-result images*, whose payload is kilobytes spanning many cells and which
must survive `Paragraph…wrap()`. A hyperlink is a zero-width state toggle on two cells injected
*after* wrapping. The two cases are not the same and the image rationale does not extend here.

## Required implementation

Six files change. Nothing else.

### File 1 — `crates/cyrup-tools/src/path.rs` (new public fn)

Add the inverse of the existing `file_url_to_path` (path.rs:182-188) / `percent_decode`
(path.rs:191-215), next to them. This is the only cyrup-tools change.

```rust
/// Node `pathToFileURL(p).href` (`render-utils.ts:22`) — the inverse of [`file_url_to_path`].
///
/// Percent-encodes every byte outside the WHATWG *path* safe set, so the C0 controls, space, `"`,
/// `#`, `<`, `>`, `?`, `` ` ``, `{`, `}`, `%` and all non-ASCII bytes (UTF-8, byte-wise) come back
/// escaped. `/` is a separator and is preserved. On Windows the leading component is prefixed with
/// `/` so `C:\x` becomes `file:///C:/x`, matching Node.
pub fn path_to_file_url(path: &Path) -> String {
    const SAFE: &[u8] = b"-._~!$&'()*+,;=:@/";
    let raw = path.to_string_lossy();
    let raw = if cfg!(windows) { raw.replace('\\', "/") } else { raw.into_owned() };
    let mut out = String::from("file://");
    if !raw.starts_with('/') {
        out.push('/');
    }
    for &b in raw.as_bytes() {
        if b.is_ascii_alphanumeric() || SAFE.contains(&b) {
            out.push(b as char);
        } else {
            out.push('%');
            out.push_str(&format!("{b:02X}"));
        }
    }
    out
}
```

`Path` is already imported in that module. The href is built from
[`resolve_to_cwd`](../../../crates/cyrup-tools/src/path.rs) (path.rs:248-270), which is pi's
`resolvePath(rawPath, cwd)` — the exact function `linkPath` uses.

### File 2 — `crates/cyrup-tui/src/osc.rs` (new module)

Register `mod osc;` in [lib.rs](../../../crates/cyrup-tui/src/lib.rs) between `mod open_browser;`
(`:70`) and `mod overlay;` (`:71`).

The module owns three things: the marker channel, the escape strings, and the buffer pass.

**The marker channel.** `Modifier` is `bitflags!` over a `u16` with nine defined bits
(`ratatui-core-0.1.2/src/style.rs:105-113`, `bitflags 2.12`). Bits 9..15 are unallocated, and the
crossterm backend's `ModifierDiff::queue` (`ratatui-crossterm-0.1.2/src/lib.rs` — a chain of
`removed.contains(KNOWN)` / `added.contains(KNOWN)` tests) emits **nothing** for an unknown bit.
`Cell::set_style` carries them in via `self.modifier.insert(style.add_modifier)`
(`cell.rs:/set_style/`). That is a clean, invisible, style-preserving side channel from `Span` to
`Cell` through `Paragraph`'s wrapper — the one thing the escape itself cannot do.

```rust
//! OSC-8 hyperlink emission — Pi `hyperlink(text, url)` (`packages/tui/src/…`), gated on
//! `getCapabilities().hyperlinks` (`terminal-image.ts:130-143`). The module
//! `app/state.rs:152` already names.
//!
//! ## Why the escape is not in the `Span`
//!
//! `Span::styled_graphemes` filters `char::is_control` (ratatui-core `text/span.rs:311-317`), so an
//! `ESC` in span text is deleted and `]8;;…` lands in the transcript as visible garbage; and
//! `Span::width` would count those bytes as columns, corrupting `wrap_line`, `wrapped_height` and
//! the content-sized inline viewport. The escape therefore goes into the `Buffer` **cell**, which
//! `CrosstermBackend::draw` prints verbatim, with `CellDiffOption::ForcedWidth` restoring the true
//! column count for the diff — the mechanism `ratatui-image` uses for Kitty/iTerm2/sixel
//! (`ratatui-image-11.0.6/src/protocol.rs:31`).
//!
//! ## How the renderer says "these cells are a link"
//!
//! `Modifier` has seven unallocated bits (`ratatui-core/src/style.rs:105-113`). A link is stamped
//! into bits 9..15 as a 1..=127 id, assigned **cyclically in render order**, so two links that end
//! up adjacent in reading order always carry different ids and can never merge into one run. The
//! crossterm backend emits nothing for unknown modifier bits, and [`inject`] clears them before the
//! frame is diffed, so the marker is unobservable either way.

use ratatui::buffer::{Buffer, CellDiffOption};
use ratatui::style::{Modifier, Style};
use std::cell::RefCell;
use std::num::NonZeroU16;

/// Bits 9..15 of `Modifier`, the seven the enum leaves unallocated.
const LINK_MASK: u16 = 0b1111_1110_0000_0000;
const LINK_SHIFT: u32 = 9;
const MAX_ID: u16 = 127;

/// One cell wide regardless of how many bytes of escape the symbol carries.
const UNIT_WIDTH: CellDiffOption = match NonZeroU16::new(1) {
    Some(w) => CellDiffOption::ForcedWidth(w),
    None => CellDiffOption::None,
};

/// The hrefs registered during one render pass, in assignment order. Held behind a `RefCell` so it
/// can ride on the `Copy` per-paint bag (`ImageOpts`) instead of threading an `&mut` through every
/// tool renderer.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct LinkSink {
    urls: RefCell<Vec<String>>,
}

impl LinkSink {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Register `url` and return the [`Style`] marker that tags its cells. Ids cycle 1..=127; the
    /// href table grows without bound within a pass, so `id` indexes it modulo 127 on read.
    pub(crate) fn mark(&self, url: String) -> Style {
        let mut urls = self.urls.borrow_mut();
        urls.push(url);
        let id = ((urls.len() - 1) as u16 % MAX_ID) + 1;
        Style::default().add_modifier(Modifier::from_bits_retain(id << LINK_SHIFT))
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.urls.borrow().is_empty()
    }

    fn url_for(&self, id: u16, seen: usize) -> Option<String> {
        // `seen` counts runs already resolved, so the n-th run of id `k` is the n-th href whose
        // slot congruates to `k` — the inverse of `mark`'s cyclic assignment.
        let urls = self.urls.borrow();
        let mut idx = (id as usize).checked_sub(1)?;
        idx += seen.saturating_mul(MAX_ID as usize);
        urls.get(idx).cloned()
    }
}

/// `\x1b]8;;<url>\x07` — OSC-8 open, BEL-terminated (the form pi emits,
/// `login-dialog.ts:98-104`, and the form `ansi::strip_ansi` recognises).
fn open(url: &str) -> String {
    format!("\u{1b}]8;;{url}\u{7}")
}

/// `\x1b]8;;\x07` — OSC-8 close.
const CLOSE: &str = "\u{1b}]8;;\u{7}";

/// Read the link id a cell carries, or `None`.
fn id_of(modifier: Modifier) -> Option<u16> {
    let id = (modifier.bits() & LINK_MASK) >> LINK_SHIFT;
    (id != 0).then_some(id)
}

/// Wrap every marked run of cells in `buf` in its OSC-8 escape, and strip the marker bits.
///
/// Walks `buf.content` in **reading order**, which is what makes a link that word-wrapped across a
/// row boundary come out as one run: the terminal's hyperlink attribute is state, not a per-cell
/// property, so the open on the run's first cell stays in force across the intervening `MoveTo`
/// until the close on its last.
///
/// A no-op when `sink` is empty, so the whole path costs one `is_empty()` on a hyperlink-incapable
/// terminal.
pub(crate) fn inject(buf: &mut Buffer, sink: &LinkSink) {
    if sink.is_empty() {
        return;
    }
    let mut i = 0usize;
    let mut seen = 0usize;
    while i < buf.content.len() {
        let Some(id) = buf.content.get(i).and_then(|c| id_of(c.modifier)) else {
            i += 1;
            continue;
        };
        let start = i;
        while buf
            .content
            .get(i)
            .and_then(|c| id_of(c.modifier))
            .is_some_and(|next| next == id)
        {
            i += 1;
        }
        let end = i - 1;
        let url = sink.url_for(id, seen);
        seen += 1;
        // Clear the marker before anything can observe it, run resolved or not.
        for cell in buf.content.get_mut(start..=end).into_iter().flatten() {
            cell.modifier = Modifier::from_bits_retain(cell.modifier.bits() & !LINK_MASK);
        }
        let Some(url) = url else { continue };
        if let Some(cell) = buf.content.get_mut(start) {
            let symbol = format!("{}{}", open(&url), cell.symbol());
            cell.set_symbol(&symbol).set_diff_option(UNIT_WIDTH);
        }
        if let Some(cell) = buf.content.get_mut(end) {
            let symbol = format!("{}{CLOSE}", cell.symbol());
            cell.set_symbol(&symbol).set_diff_option(UNIT_WIDTH);
        }
    }
}
```

`ForcedWidth(1)` is correct for the head and tail cells because a marked run is only ever produced
from `shorten_path` output, which the wrapper lays down one grapheme per cell; a wide grapheme
occupies one buffer slot with its trailing slot blanked, and `diff_iter`'s `ForcedWidth` arm
advances one slot exactly as `CellDiffOption::None` would for that leading cell.

### File 3 — `crates/cyrup-tui/src/transcript/tool_render.rs`

Add the sink and the capability to the per-paint bag, which
[tool_render.rs:95-138](../../../crates/cyrup-tui/src/transcript/tool_render.rs) already describes
as "the per-paint bag for everything an `Entry` cannot carry on itself". `ImageOpts` derives
`Clone, Copy, Debug, PartialEq, Eq`; `&LinkSink` is `Copy`, and `LinkSink` derives
`Debug, Default, PartialEq, Eq`, so every derive still holds.

CURRENT (`:107` and `:123`, and the `Default` impl at `:126-138`):

```rust
    /// Pi `ToolRenderContext.cwd` — the SESSION's working directory, not necessarily the process's.
    /// `None` falls back to the process cwd.
    pub cwd: Option<&'a std::path::Path>,
```

REPLACEMENT — add two fields after `cwd`:

```rust
    /// Pi `ToolRenderContext.cwd` — the SESSION's working directory, not necessarily the process's.
    /// `None` falls back to the process cwd.
    pub cwd: Option<&'a std::path::Path>,
    /// `getCapabilities().hyperlinks` (`render-utils.ts:20`) — whether the controlling terminal
    /// forwards OSC-8. Threaded rather than read from `crate::image::hyperlinks_supported()` for
    /// the same reason `graphical` is (TUI-N01/TUI-N11): the global falls back to an env sniff, so
    /// reading it here would make every header assertion depend on the developer's `TERM_PROGRAM`.
    pub hyperlinks: bool,
    /// Where [`tool_path_span`] registers an href for [`crate::osc::inject`] to emit. `None` on any
    /// path that does not own the resulting `Buffer` — the link is then simply not marked.
    pub links: Option<&'a crate::osc::LinkSink>,
```

and in `impl Default for ImageOpts<'_>` add `hyperlinks: false, links: None,`. `false` is pi's own
conservative default (`terminal-image.ts:130-134`) and keeps every existing construction of
`ImageOpts::default()` on the plain-text branch.

### File 4 — `crates/cyrup-tui/src/transcript/tool_args.rs`

CURRENT ([tool_args.rs:26-43](../../../crates/cyrup-tui/src/transcript/tool_args.rs)):

```rust
/// `renderToolPath` (render-utils.ts:75-85): `[invalid arg]` for a non-string, the `emptyFallback`
/// (else `...`) for an empty/absent path, otherwise the `~`-shortened path in accent. Hyperlinks are a
/// terminal escape the cell grid does not carry (tracked residual).
pub(super) fn tool_path_span(
    args: &Value,
    keys: &[&str],
    empty_fallback: Option<&str>,
    theme: &UiTheme,
) -> Span<'static> {
    match str_arg(args, keys) {
        StrArg::Invalid => Span::styled("[invalid arg]".to_string(), theme.error_style()),
        StrArg::Missing => match empty_fallback {
            Some(f) => Span::styled(shorten_path(f), theme.accent_style()),
            None => Span::styled("...".to_string(), theme.tool_output_style()),
        },
        StrArg::Value(p) => Span::styled(shorten_path(&p), theme.accent_style()),
    }
}
```

REPLACEMENT:

```rust
/// `renderToolPath` (render-utils.ts:75-85): `[invalid arg]` for a non-string, the `emptyFallback`
/// (else `...`) for an empty/absent path, otherwise the `~`-shortened path in accent — wrapped in an
/// OSC-8 hyperlink to the resolved path's `file://` URL when the terminal forwards them
/// (`linkPath`, `:19-23`).
///
/// The gate and the two unlinked arms are upstream's, exactly: `linkPath` is reached only from
/// `:84`'s `accent` branch, so `invalidArgText` (`:71-73`) and the `toolOutput` `...` (`:83`) stay
/// inert; and the href is built from `value` — the RAW path — while the visible text is the
/// `~`-shortened form, because `shortenPath` is display-only.
///
/// The escape itself is not in this `Span`. [`crate::osc`] explains why (`Span::styled_graphemes`
/// deletes `ESC`, and `Span::width` would miscount the rest as columns); the span carries a marker
/// in `Modifier`'s unallocated bits and [`crate::osc::inject`] converts marked cells into the
/// escape once the `Buffer` exists.
pub(super) fn tool_path_span(
    args: &Value,
    keys: &[&str],
    empty_fallback: Option<&str>,
    theme: &UiTheme,
    opts: ImageOpts<'_>,
) -> Span<'static> {
    match str_arg(args, keys) {
        StrArg::Invalid => Span::styled("[invalid arg]".to_string(), theme.error_style()),
        StrArg::Missing => match empty_fallback {
            Some(f) => Span::styled(shorten_path(f), link_style(f, theme, opts)),
            None => Span::styled("...".to_string(), theme.tool_output_style()),
        },
        StrArg::Value(p) => Span::styled(shorten_path(&p), link_style(&p, theme, opts)),
    }
}

/// `theme.fg("accent", …)`, plus [`crate::osc`]'s link marker when the terminal forwards OSC-8 —
/// `linkPath(styledText, rawPath, cwd)` (`render-utils.ts:19-23`) with pi's own early return:
///
/// ```ts
/// if (!getCapabilities().hyperlinks) return styledText;
/// const absolutePath = resolvePath(rawPath, cwd);
/// return hyperlink(styledText, pathToFileURL(absolutePath).href);
/// ```
///
/// `resolvePath(rawPath, cwd)` is `cyrup_tools::path::resolve_to_cwd`, the same port `read`'s
/// compact classification resolves through (`read.ts:336`). A `cwd` of `None` falls back to the
/// process cwd, matching [`compact_read_classification`]; if even that is unavailable the path
/// cannot be resolved and the span stays unlinked rather than pointing somewhere wrong.
fn link_style(raw_path: &str, theme: &UiTheme, opts: ImageOpts<'_>) -> Style {
    let accent = theme.accent_style();
    if !opts.hyperlinks {
        return accent;
    }
    let Some(sink) = opts.links else { return accent };
    let base = match opts.cwd {
        Some(c) => c.to_path_buf(),
        None => match std::env::current_dir() {
            Ok(c) => c,
            Err(_) => return accent,
        },
    };
    let absolute = cyrup_tools::path::resolve_to_cwd(raw_path, &base);
    let url = cyrup_tools::path::path_to_file_url(&absolute);
    accent.patch(sink.mark(url))
}
```

`Style` and `ImageOpts` reach this module through its existing `use super::*;`.

### File 5 — `crates/cyrup-tui/src/transcript/tool_builtin.rs`

All four call sites gain `opts`. `render_read` and `render_ls` already hold an `ImageOpts` named
`opts`; `render_write` and `render_edit` do not take one yet and must — add
`opts: ImageOpts<'_>` to their signatures and pass it from
[tool_render.rs](../../../crates/cyrup-tui/src/transcript/tool_render.rs)'s dispatch, which already
holds `images`.

| line | CURRENT | REPLACEMENT |
| --- | --- | --- |
| `:21` (`read`) | `spans.push(tool_path_span(&run.args, &["file_path", "path"], None, theme));` | `spans.push(tool_path_span(&run.args, &["file_path", "path"], None, theme, opts));` |
| `:71` (`write`) | `spans.push(tool_path_span(&run.args, &["file_path", "path"], None, theme));` | `spans.push(tool_path_span(&run.args, &["file_path", "path"], None, theme, opts));` |
| `:170` (`edit`) | `spans.push(tool_path_span(&run.args, &["file_path", "path"], None, theme));` | `spans.push(tool_path_span(&run.args, &["file_path", "path"], None, theme, opts));` |
| `:374` (`ls`) | `spans.push(tool_path_span(&run.args, &["path"], Some("."), theme));` | `spans.push(tool_path_span(&run.args, &["path"], Some("."), theme, opts));` |

`push_search_path` (`:325`, `:355`) is **unchanged** — grep/find do not link upstream.

### File 6a — `crates/cyrup-tui/src/app/draw.rs` (the scrollback flush)

CURRENT ([draw.rs:141-152](../../../crates/cyrup-tui/src/app/draw.rs), inside the `ImageOpts`
literal), then `:174-183`:

```rust
        let images = crate::transcript::ImageOpts {
            show: self.state.transcript.show_images(),
            graphical: self.state.transcript.graphical_images(),
            width_cells: self.state.transcript.image_width_cells(),
            expand_key: self.state.transcript.expand_key(),
            cwd: self.state.transcript.cwd(),
```
```rust
        let height = crate::transcript::wrapped_height(&lines, width).min(u16::MAX as usize) as u16;
        self.terminal
            .insert_before(height, move |buf| {
                Paragraph::new(lines).style(style).wrap(Wrap { trim: false }).render(buf.area, buf);
            })
            .map_err(|e| TuiError::Backend(e.to_string()))?;
```

REPLACEMENT — build the sink before `entry_lines` runs, hand it to the bag, and inject after the
`Paragraph` has written the temp buffer:

```rust
        // TUI-020 — the hrefs `tool_path_span` registers while `entry_lines` runs, emitted as
        // OSC-8 once the cells exist. Built per flush; empty on a hyperlink-incapable terminal, in
        // which case `osc::inject` returns on its first line.
        let links = crate::osc::LinkSink::new();
        let images = crate::transcript::ImageOpts {
            show: self.state.transcript.show_images(),
            graphical: self.state.transcript.graphical_images(),
            width_cells: self.state.transcript.image_width_cells(),
            expand_key: self.state.transcript.expand_key(),
            cwd: self.state.transcript.cwd(),
            hyperlinks: self.state.transcript.hyperlinks(),
            links: Some(&links),
```
```rust
        let height = crate::transcript::wrapped_height(&lines, width).min(u16::MAX as usize) as u16;
        self.terminal
            .insert_before(height, move |buf| {
                Paragraph::new(lines).style(style).wrap(Wrap { trim: false }).render(buf.area, buf);
                // AFTER the wrap: the escape must not be present while `Paragraph` measures
                // columns, and the marked cells do not exist until it has written them.
                crate::osc::inject(buf, &links);
            })
            .map_err(|e| TuiError::Backend(e.to_string()))?;
```

`links` moves into the `move` closure, which runs once, synchronously, inside `insert_before` — the
`lines` binding already moves the same way.

`AppState::scrollback`'s clone at `:166-167` stays where it is, above this, so the accumulator keeps
holding escape-free `Line`s.

### File 6b — `crates/cyrup-tui/src/transcript/cache.rs` + `mod.rs` + `view.rs` (the live viewport)

An in-flight tool renders live before it commits ([cache.rs:130-149](../../../crates/cyrup-tui/src/transcript/cache.rs)),
so the live path needs the same treatment or a `read` header becomes clickable only after it
scrolls away.

`RenderCache` ([mod.rs:203-209](../../../crates/cyrup-tui/src/transcript/mod.rs)) gains the sink so
it is rebuilt with — and invalidated with — the lines it belongs to:

```rust
struct RenderCache {
    generation: u64,
    width: usize,
    theme_generation: u64,
    lines: Vec<Line<'static>>,
    wrapped_height: usize,
    /// The hrefs `lines` was built with (TUI-020). Cached alongside because the ids in the spans'
    /// marker bits index THIS table; a cache hit that reused stale hrefs would link the right text
    /// to the wrong file.
    links: crate::osc::LinkSink,
}
```

`TranscriptView::lines` (mod.rs / cache.rs:70) takes `&LinkSink` and passes it as
`links: Some(sink)` in the `ImageOpts` it builds at cache.rs:139-149, alongside
`hyperlinks: self.hyperlinks`. `cached_render` constructs the sink, calls `lines`, and stores both.

`TranscriptView::render` ([cache.rs:169-188](../../../crates/cyrup-tui/src/transcript/cache.rs))
injects after the widget writes:

CURRENT:

```rust
        let para = Paragraph::new(lines)
            .style(theme.base_style())
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0));
        frame.render_widget(para, area);
```

REPLACEMENT:

```rust
        let para = Paragraph::new(lines)
            .style(theme.base_style())
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0));
        frame.render_widget(para, area);
        // TUI-020 — same ordering rule as the scrollback flush: inject only once the cells exist.
        // `ForcedWidth` keeps `Buffer::diff_iter` advancing one column per escaped cell, so the
        // frame-to-frame diff that drives the live viewport stays aligned.
        crate::osc::inject(frame.buffer_mut(), &self.render_cache.links);
```

Take the `links` clone or reborrow before the `&mut self` render borrow if the borrow checker
objects; `cached_render` is already called above it and the pattern there is "check, build, assign,
lend".

Finally, the capability field on `TranscriptView` — mirroring `graphical_images`
([mod.rs:156](../../../crates/cyrup-tui/src/transcript/mod.rs),
[view.rs:46-57](../../../crates/cyrup-tui/src/transcript/view.rs)):

```rust
    /// `getCapabilities().hyperlinks` (`terminal-image.ts:130-143`). Boot default **false**, pi's
    /// own conservative value; refined once by `App::detect_image_support`. Held here rather than
    /// read from `crate::image::hyperlinks_supported()` at paint time because that getter falls
    /// back to an env sniff — the TUI-N11 hermeticity hole (`image.rs:508-546`).
    hyperlinks: bool,
```

with `set_hyperlinks` / `hyperlinks()` beside `set_graphical_images` / `graphical_images`, and one
line added to [shell.rs:407-409](../../../crates/cyrup-tui/src/app/shell.rs) where the detection is
already published:

```rust
        self.state
            .transcript
            .set_graphical_images(self.state.image_renderer.is_graphical());
        // Feature #8 — the same publish for the OSC-8 gate `tool_path_span` reads (TUI-020).
        self.state.transcript.set_hyperlinks(caps.hyperlinks);
```

`set_hyperlinks` must call `bump_render_generation()`, exactly as `set_graphical_images` does, so a
cache built before detection is discarded.

## Scope boundaries

* **Only** the four `renderToolPath` headers become links. `push_search_path`,
  `compact_read_call`, the markdown link arm and `login_dialog` are all out of scope; each is its
  own upstream shape and its own ledger item.
* No ` (url)` fallback is added anywhere. `renderToolPath` has none, and inventing one would be
  behaviour pi does not have.
* `crates/cyrup-tui/src/image.rs`'s half-block rationale and `crates/cyrup-tui/src/ansi.rs` are not
  edited. The sanitizer never sees a header span.

## Genuinely uncertain

* **`Modifier`'s spare bits are an unallocated implementation detail of ratatui.** They are stable
  in `0.30.x` (nine flags in a `u16`), but a future ratatui that allocates bit 9 would silently turn
  a link marker into a real attribute. The mask is a single constant in `crate::osc`, and the
  cheapest defence is a compile-time guard next to it:
  `const _: () = assert!(Modifier::all().bits() & LINK_MASK == 0);` — a build break rather than a
  visual one.
* **`pathToFileURL` byte-fidelity.** Node's encoder is the WHATWG path percent-encode set plus a
  pre-pass that escapes `%`, `#`, `?`, `\n`, `\r`, `\t`. The set prescribed above is a superset for
  ASCII, so an exotic path could produce a href that is *more* escaped than pi's. Terminals decode
  both to the same file, so nothing observable diverges; it is a fidelity caveat, not a defect.
* **Terminals that advertise OSC-8 but mishandle a link split across a wrap.** The escape is opened
  once and closed once, with the run crossing a row boundary in between. This is the standard
  behaviour every OSC-8 emitter relies on, but a terminal with a per-row hyperlink state machine
  would drop the tail. The gate already excludes unidentified terminals
  ([image.rs:703-705](../../../crates/cyrup-tui/src/image.rs)), which bounds the exposure.
* **`ratatui-image` and `crate::osc` both write raw symbols into the same buffer.** They never
  target the same cells (images are rasterised as half-blocks in the transcript,
  [image.rs:342-350](../../../crates/cyrup-tui/src/image.rs)), and the attachment strip is a
  different widget in a different rect. Worth keeping in mind if the transcript ever gains the
  negotiated image protocol.

## Definition of done

1. On a terminal that forwards OSC-8, the path in a `read`, `write`, `edit` or `ls` tool header is a
   clickable target that opens the file the tool acted on — both while the tool is live in the
   inline viewport and after it has flushed to native scrollback.
2. The href is `file://` + the percent-encoded **absolute** path produced by resolving the raw
   argument against the session cwd; the **visible** text remains the `~`-shortened form, unchanged
   in content, colour and column width from today.
3. `ls` with no `path` argument shows `.` and links to the session cwd. `[invalid arg]` and the
   `...` placeholder carry no link.
4. The `" in <path>"` tail of a `grep` or `find` header, the compact `read resource` / `[skill]`
   labels, and markdown link text are all still inert — no escape reaches any of them.
5. On a terminal that does not advertise OSC-8, and on every non-terminal render path, the emitted
   bytes are identical to today: no `ESC`, no `]8;;`, no trailing ` (url)`.
6. No `]8;;`, `file://`, or other escape residue is ever visible as text anywhere in the transcript,
   at any terminal width, including when a header path word-wraps across rows.
7. Column positions elsewhere on a header row — the `:12-40` line range after a `read` path, the
   ` (limit N)` after an `ls` path — are unmoved by the presence of a link, and the inline
   viewport's content height for a turn containing a linked header equals its height without one.
8. Tool-result text still has its own ANSI stripped: an `ls` whose output contains
   `\x1b]8;;file:///tmp/x\x07linked\x1b]8;;\x07` still renders `linked` with no payload visible.
9. Behaviour pi does not have is not introduced — this is a parity task, not a redesign.
