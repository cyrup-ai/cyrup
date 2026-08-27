---
stage: exec
status: in-progress
priority: LOW
tool: all
source: aug follow-up from the OSC-8 regression-test task
updated: 2026-08-27 20:36
---

# `osc::inject`'s `ForcedWidth(1)` understates a wide head/tail grapheme

[`crates/cyrup-tui/src/osc.rs`](../../../crates/cyrup-tui/src/osc.rs) defines `UNIT_WIDTH =
CellDiffOption::ForcedWidth(1)` (`osc.rs:67-71`) and stamps it on the **head** and **tail** cell of
every marked run inside `inject` (`osc.rs:160-167`). When either of those cells holds a wide
grapheme — a two-column CJK character, an emoji — the forced value understates its real column
count by one, and `Buffer::diff_iter` stops skipping the grapheme's continuation cell.

---

## 1 — The real ratatui API, verified against the pinned source

Pinned at `Cargo.lock:5543-5546`: **`ratatui-core` 0.1.2**, checksum
`cbb175c433c8e28a809d1f5773a2ae96e68c0ce40db865cbab1020bf33ae479c`, vendored at
`/root/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/ratatui-core-0.1.2`.

**The trait exists and the original prescription's path was right.**
`/root/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/ratatui-core-0.1.2/src/buffer/cell_width.rs:19-22`:

```rust
pub trait CellWidth {
    /// Returns the display width in terminal cells.
    fn cell_width(&self) -> u16;
}
```

`impl CellWidth for str` is at `cell_width.rs:24-46` (ASCII fast path, then `UnicodeWidthStr::width`
plus a halfwidth-dakuten correction). It is re-exported unconditionally:

- `ratatui-core-0.1.2/src/buffer.rs:12` — `pub use cell_width::CellWidth;`
- `/root/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/ratatui-0.30.2/src/lib.rs:480` —
  `pub use ratatui_core::{buffer, layout};`

So **`ratatui::buffer::CellWidth`** and **`ratatui::buffer::Cell`** both resolve from this crate with
no new dependency and no feature gate. Confirmed by three in-tree users of the same import:
`ratatui-core-0.1.2/src/text/span.rs:8`, `src/backend/test.rs:10`, `src/buffer/diff.rs:1`.

**`impl CellWidth for Cell`** — `ratatui-core-0.1.2/src/buffer/cell.rs:309-318`:

```rust
impl CellWidth for Cell {
    /// Returns [`CellDiffOption::ForcedWidth`] when set, otherwise computes the width from the
    /// cell's symbol.
    fn cell_width(&self) -> u16 {
        match self.diff_option {
            CellDiffOption::ForcedWidth(w) => w.get(),
            _ => self.symbol().cell_width(),
        }
    }
}
```

Also verified, and load-bearing below:

| Symbol | Path | Lines |
| --- | --- | --- |
| `pub enum CellDiffOption` / `ForcedWidth(NonZeroU16)` | `ratatui-core-0.1.2/src/buffer/cell.rs` | 12-32 / 31 |
| `pub fn set_symbol(&mut self, &str) -> &mut Self` | `ratatui-core-0.1.2/src/buffer/cell.rs` | 156 |
| `pub const fn set_diff_option(&mut self, CellDiffOption) -> &mut Self` | `ratatui-core-0.1.2/src/buffer/cell.rs` | 241 |
| `Cell::symbol` (`None` reads back as `" "`) | `ratatui-core-0.1.2/src/buffer/cell.rs` | 105-107 |
| `impl PartialEq for Cell` — compares `diff_option` | `ratatui-core-0.1.2/src/buffer/cell.rs` | 253-278 (277) |
| `Buffer::set_stringn` — resets the continuation cells of a wide grapheme | `ratatui-core-0.1.2/src/buffer/buffer.rs` | 336-368 (363-366) |
| `Buffer::diff_iter` | `ratatui-core-0.1.2/src/buffer/buffer.rs` | 506-508 |
| `Terminal` flush → `previous.diff_iter(current)` | `ratatui-core-0.1.2/src/terminal/buffers.rs` | 97-107 |
| `insert_before` → `old.diff_iter(&new)` | `ratatui-core-0.1.2/src/terminal/inline.rs` | 340 |
| `impl Backend for TestBackend` — `draw` **clones the whole cell**, `diff_option` included | `ratatui-core-0.1.2/src/backend/test.rs` | 252-259 |

### How `diff_iter` actually consumes the value

`ratatui-core-0.1.2/src/buffer/diff.rs:122-141` — `self.pos` is incremented by 1 at the top of the
loop, then the `ForcedWidth` arm adds `width - 1`:

```rust
while self.pos < len {
    let i = self.pos;
    self.pos += 1;
    ...
    CellDiffOption::ForcedWidth(width) => {
        self.pos = self
            .pos
            .saturating_add(width.get().saturating_sub(1) as usize);
        if current != previous {
            let (x, y) = self.pos_of(i);
            return Some((x, y, &self.next[i]));
        }
    }
```

**`ForcedWidth(w)` means "this cell owns `w` slots of the flat content array".** Total advance is
exactly `w`. That is the same accounting the un-forced path performs for an ordinary wide grapheme
at `diff.rs:172-173`:

```rust
} else if cell_width > 1 {
    self.pos += cell_width.saturating_sub(1);
```

So the correct forced value is **the number of terminal columns the cell physically occupies** —
which, because OSC-8 escapes are zero-column, is the width of the **original grapheme**, not of the
mutated symbol.

---

## 2 — The defect, stated exactly

`box_lines` (`transcript/layout.rs:139-174`, always called with `padding_x = 1`) wraps the tool
header, and `apply_bg` (`layout.rs:183`) right-pads it. The marked run therefore ends at the last
grapheme of the path span. `read /tmp/aug-osc/プロジェクト` puts a **two-column `ト` in the tail
cell** with no wrapping at all; a wrap additionally puts a wide grapheme in the **head** cell of the
second row, because `wrap_line`'s long-word arm walks one grapheme at a time (`layout.rs:78-89`) and
never splits a cluster.

For a wide grapheme at flat index `i`, `set_stringn` writes the symbol at `i` and `reset()`s `i+1`
(`buffer.rs:363-366`), so `i+1` is `Cell::EMPTY`. With `ForcedWidth(1)` the iterator advances to
`i+1` and evaluates it as a real cell. Against a previous frame that held content there — the
`insert_before` scrollback flush at [`app/draw.rs:191`](../../../crates/cyrup-tui/src/app/draw.rs)
goes through `inline.rs:340` with a *non-empty* `old` buffer — it yields `(x+1, y, EMPTY)`, and
`CrosstermBackend` then emits `MoveTo(x+1, y)` + `Print(" ")`, erasing the right half of the wide
grapheme. Every subsequent column on that row is also one slot out of step with what the un-forced
render would have produced.

---

## 3 — The original prescription is WRONG for the tail cell. Do not implement it.

The parity note this task was filed from said to capture `cell.symbol().cell_width()`. That is
correct for the head cell and **actively harmful for the tail cell**, because `inject` runs head
first and tail second on the *same* cell when a run is one cell long (`osc.rs:158-167`, and the
comment there says so):

```rust
// Head first, then tail — a one-cell run is `start == end`, and doing it in this order
// leaves that single cell holding `open + symbol + CLOSE`.
```

By the time the tail branch reads `cell.symbol()`, that symbol is already `open(url) + grapheme`.
`str::cell_width` on `"\u{1b}]8;;file:///tmp/aug-osc\u{7}."` measures the **28 printable bytes of the
escape** (`\u{1b}` and `\u{7}` are zero-width to `unicode-width`; the rest are ASCII), so the tail
branch would force `ForcedWidth(28)` and `diff_iter` would swallow 27 real cells.

One-cell runs are live today: `ls` with no `path` renders the `Some(".")` fallback
(`transcript/tool_args.rs:50-51`), a single cell, and two of the nine existing tests paint it
(clause 3 and clause 8). Those tests would still pass — the 27 swallowed cells are all `apply_bg`
padding whose symbol is `" "` either way — which is precisely why this must be caught here and not
by the suite.

**Read `cell.cell_width()`, not `cell.symbol().cell_width()`.** `Cell::cell_width` returns an
already-set `ForcedWidth` verbatim (`cell.rs:313-314`), so:

- head branch, cell untouched → `diff_option` is `None` → falls through to `symbol().cell_width()` →
  the original grapheme's width. Correct.
- tail branch, `start != end` → same. Correct.
- tail branch, `start == end` → returns the value the head branch just forced. Correct, and immune
  to the escape.

---

## 4 — Required change (single path)

### 4.1 `crates/cyrup-tui/src/osc.rs` — imports

```rust
use ratatui::buffer::{Buffer, Cell, CellDiffOption, CellWidth};
```

`NonZeroU16` stays imported (`osc.rs:54`); `Modifier`, `Style`, `RefCell` are unchanged.

### 4.2 Replace the `UNIT_WIDTH` constant (`osc.rs:67-71`)

Delete:

```rust
/// One cell wide regardless of how many bytes of escape the symbol carries.
const UNIT_WIDTH: CellDiffOption = match NonZeroU16::new(1) {
    Some(w) => CellDiffOption::ForcedWidth(w),
    None => CellDiffOption::None,
};
```

Add, in its place:

```rust
/// The column count a cell keeps once its symbol carries an OSC-8 escape.
///
/// OSC-8 is zero-column, so the answer is always the width the cell had *before* [`inject`] touched
/// it — one for ASCII, two for a CJK ideograph or an emoji. `diff_iter` treats `ForcedWidth(w)` as
/// "this cell owns `w` slots" (`ratatui-core-0.1.2/src/buffer/diff.rs:133-141`), which is exactly
/// the advance the un-forced path gives a wide grapheme (`diff.rs:172-173`), so forcing the true
/// width makes a linked run diff identically to an unlinked one.
///
/// Read through [`CellWidth::cell_width`] on the **cell**, never on `cell.symbol()`: `Cell` returns
/// an already-set `ForcedWidth` verbatim (`ratatui-core-0.1.2/src/buffer/cell.rs:309-318`), so on
/// the tail of a one-cell run — where the head branch has already prepended the escape — this
/// yields the head's forced width instead of re-measuring ~28 columns of escape text.
///
/// Width `0` (a zero-width symbol) maps to `1`, which is the advance `diff_iter` already gives such
/// a cell on the `CellDiffOption::None` path (`diff.rs:149-153`); `ForcedWidth` cannot hold `0`.
fn forced_width(cell: &Cell) -> CellDiffOption {
    CellDiffOption::ForcedWidth(NonZeroU16::new(cell.cell_width()).unwrap_or(NonZeroU16::MIN))
}
```

`NonZeroU16::MIN` rather than `NonZeroU16::new(1).unwrap()` — `clippy::unwrap_used` and
`clippy::expect_used` are `deny` for every workspace member (`Cargo.toml:98-99`), and `osc.rs`
carries no `allow`.

### 4.3 The two call sites in `inject` (`osc.rs:158-167`)

The width must be read **before** `set_symbol`. Keep the head-then-tail order and its comment —
it is what makes the `start == end` case work.

```rust
        // Head first, then tail — a one-cell run is `start == end`, and doing it in this order
        // leaves that single cell holding `open + symbol + CLOSE`.
        if let Some(cell) = buf.content.get_mut(start) {
            let width = forced_width(cell);
            let symbol = format!("{}{}", open(&url), cell.symbol());
            cell.set_symbol(&symbol).set_diff_option(width);
        }
        if let Some(cell) = buf.content.get_mut(end) {
            let width = forced_width(cell);
            let symbol = format!("{}{CLOSE}", cell.symbol());
            cell.set_symbol(&symbol).set_diff_option(width);
        }
```

`forced_width(cell)` reborrows the `&mut Cell` as `&Cell` and returns a `Copy` value
(`CellDiffOption` derives `Copy` at `cell.rs:10`), so the borrow ends before `set_symbol`.

`CellDiffOption::Skip` remains unused — `diff_iter` drops skipped cells outright
(`diff.rs:130`, `is_skip` at `diff.rs:202-207`) and that would delete the escape.

### 4.4 Module doc

`osc.rs:16-22` still reads correctly (`ForcedWidth` is still mandatory, `Skip` still forbidden).
Amend only the sentence naming the constant, if the implementer's wording references `UNIT_WIDTH`.

---

## 5 — Effect on the nine existing tests

All nine live in
[`crates/cyrup-tui/src/transcript/tests/osc_hyperlinks.rs`](../../../crates/cyrup-tui/src/transcript/tests/osc_hyperlinks.rs).
Every one of them observes the render through `paint` (`osc_hyperlinks.rs:61-80`), which
concatenates `cell.symbol()` and nothing else. **`forced_width` changes no symbol**, so no assertion
in the module can move unless the diff stream changes which cells reach `TestBackend`.

Head/tail graphemes across all nine, enumerated:

| Test | Head / tail grapheme of each run | Width today → after |
| --- | --- | --- |
| `a_read_header_carries_the_open_and_close_escapes` | `/` … `s` | 1 → 1 |
| `the_href_is_the_raw_path_percent_encoded_and_the_text_is_shortened` | `~` … `s` (`é` is interior, and one column either composed or decomposed) | 1 → 1 |
| `ls_links_the_session_cwd_and_the_two_unlinked_arms_emit_no_escape` | `.` — a **one-cell run**, `start == end` | 1 → 1 |
| `grep_find_tails_and_the_compact_read_header_stay_unlinked` | no run at all (`sink.is_empty()` early-out, `osc.rs:133-135`) | n/a |
| `the_gate_off_buffer_is_byte_identical_to_today` | `/` … `s` | 1 → 1 |
| `a_wrapped_path_emits_one_pair_per_row_with_the_same_href` | two runs, all four boundary cells ASCII | 1 → 1 |
| `two_links_in_one_pass_resolve_to_distinct_hrefs` | two runs, `/` … `s` each | 1 → 1 |
| `a_linked_header_sits_above_a_result_body_whose_own_osc_8_was_stripped` | `.` — a **one-cell run** | 1 → 1 |
| `there_is_no_visible_url_fallback` | four runs, `/` … `s` / `c` each | 1 → 1 |

Every path fixture is ASCII apart from `café.rs`, whose `é` is one column and interior to the run.
`forced_width` therefore returns `ForcedWidth(1)` for every head and tail cell the nine tests
produce — **byte-identical `CellDiffOption` to today's `UNIT_WIDTH`**, hence an identical diff
stream, hence an identical `TestBackend` buffer.

The two named at-risk assertions:

- **Wrap test's `assert_eq!(strip_ansi(&linked), plain)` and its `matches(...).count() == 2` pair**
  (`osc_hyperlinks.rs:234-245`). The run walk, the id scheme, `open`/`CLOSE` and the head/tail
  ordering are all untouched; only the `CellDiffOption` argument changes, and for this fixture it
  changes to the same value. The fixture path
  `/tmp/aug-osc/a-really-long-directory-name/and-another-one/file.rs` is pure ASCII, so all four
  boundary cells of the two rows measure one column before and after.
- **`two_links_in_one_pass_resolve_to_distinct_hrefs`** (`osc_hyperlinks.rs:253-282`). Depends on
  `LinkSink::mark`'s one-based pass-unique ids (`osc.rs:88-97`) and on `url_for` (`osc.rs:104-107`).
  Neither is touched. Its `content_height` equality is a pre-`inject` property and cannot see this
  change at all.

The one-cell `ls .` runs are the only cells where head and tail alias, and §3 is exactly why they
stay at `1`.

---

## 6 — Required regression test

Append to
[`crates/cyrup-tui/src/transcript/tests/osc_hyperlinks.rs`](../../../crates/cyrup-tui/src/transcript/tests/osc_hyperlinks.rs).

**Why a symbol-only assertion cannot catch this.** On a first draw the previous buffer is all
`Cell::EMPTY`, so the continuation cell at `i+1` (also `EMPTY` after `set_stringn`'s `reset()`)
compares equal and is not yielded — with `ForcedWidth(1)` *or* `ForcedWidth(2)`. `paint`'s output
string is identical either way. The defect is in the **diff cursor**, and the only place it is
observable from a `TestBackend` is the cell's own `diff_option`, which
`<TestBackend as Backend>::draw` preserves because it clones the whole `Cell`
(`ratatui-core-0.1.2/src/backend/test.rs:252-259`). So the test must read `cell_width()`.

Add to the imports at the top of the module:

```rust
use ratatui::buffer::CellWidth;
```

And the test:

```rust
/// Clause 10 — the diff cursor, not the byte stream. `inject` forces a width on the head and tail
/// cell of every run so `Buffer::diff_iter` does not count the escape's bytes as columns
/// (`ratatui-core-0.1.2/src/buffer/diff.rs:133-141`). When that cell holds a WIDE grapheme the
/// forced value must be the grapheme's real column count, or the iterator stops skipping the
/// grapheme's continuation cell and every later column on the row is one slot out of step.
///
/// `paint` cannot see this: on a first draw the continuation cell is `Cell::EMPTY` in both the
/// previous and the next buffer, so it is never yielded whatever the forced width says. The
/// assertion has to be on `cell_width()` itself, which survives into the `TestBackend` buffer
/// because `<TestBackend as Backend>::draw` clones the whole `Cell` (`backend/test.rs:252-259`).
#[test]
fn a_wide_grapheme_at_a_run_boundary_keeps_its_true_column_count() {
    /// Every cell of the linked render must occupy exactly as many diff slots as the same cell of
    /// the unlinked render. This is "the gate adds bytes, never columns" stated for the cursor.
    fn assert_widths_match(path: &str, theme: &UiTheme, w: u16, h: u16) {
        let mut v_on = view("read", json!({ "file_path": path }), true);
        let mut on = Terminal::new(TestBackend::new(w, h)).unwrap();
        on.draw(|f| v_on.render(f, f.area(), theme)).unwrap();

        let mut v_off = view("read", json!({ "file_path": path }), false);
        let mut off = Terminal::new(TestBackend::new(w, h)).unwrap();
        off.draw(|f| v_off.render(f, f.area(), theme)).unwrap();

        let linked = on.backend().buffer();
        let plain = off.backend().buffer();
        assert_eq!(linked.area, plain.area, "the gate changed the painted area");
        let mut saw_wide = false;
        for y in 0..linked.area.height {
            for x in 0..linked.area.width {
                let (Some(l), Some(p)) = (linked.cell((x, y)), plain.cell((x, y))) else {
                    continue;
                };
                saw_wide |= p.cell_width() > 1;
                assert_eq!(
                    l.cell_width(),
                    p.cell_width(),
                    "column ({x},{y}) changed width when linked: {:?} vs {:?}",
                    l.symbol(),
                    p.symbol()
                );
            }
        }
        assert!(saw_wide, "fixture painted no wide grapheme — the test proves nothing");
    }

    let theme = UiTheme::dark();

    // (a) TAIL-wide, no wrap. `apply_bg`'s right pad is unmarked, so the run ends on the path's
    //     last grapheme — here a two-column katakana.
    let tail = "/tmp/aug-osc/プロジェクト";
    assert_widths_match(tail, &theme, 60, 12);

    // The escape really is there, and the visible columns really are unmoved.
    let mut on = view("read", json!({ "file_path": tail }), true);
    let mut off = view("read", json!({ "file_path": tail }), false);
    let linked = paint(&mut on, &theme, 60, 12);
    assert!(
        linked.contains(&format!("{}{tail}{CLOSE}", open(&format!("file://{tail}")))),
        "the wide-tailed path is not wrapped by its href:\n{linked:?}"
    );
    assert_eq!(strip_ansi(&linked), paint(&mut off, &theme, 60, 12));

    // (b) HEAD-wide, via a wrap. At width 40 `box_lines` gives content width 38; this path is
    //     13 + 15*2 = 43 columns, so `wrap_line`'s long-word arm breaks it after the 12th wide
    //     grapheme (13 + 24 = 37, the 13th would reach 39) and the next row STARTS on a wide one.
    let wrapped = format!("/tmp/aug-osc/{}", "プ".repeat(15));
    assert_widths_match(&wrapped, &theme, 40, 12);
}
```

The module's `#![allow(clippy::unwrap_used, …)]` at `osc_hyperlinks.rs:28` already covers the
`unwrap()`s, and `view` / `paint` / `open` / `CLOSE` are the module's existing helpers
(`osc_hyperlinks.rs:41-80`).

**Failure mode before the fix:** case (a) fails at the `ト` cell with `1` vs `2`; case (b) fails at
both the row-one tail and the row-two head. **After the fix:** `forced_width` returns
`ForcedWidth(2)` there and `Cell::cell_width` reports `2`, matching the unlinked cell whose
un-forced `symbol().cell_width()` is also `2`.

---

## 7 — Stale comment that must be removed

`osc_hyperlinks.rs:16-26` currently carries a `KNOWN LIMITATION, deliberately untested` block that
documents this exact defect and cites `osc.rs:68-71` — a range that ceases to exist. Replace it with
a pointer to the new clause-10 test. It is a doc comment; changing it cannot affect any assertion.

---

## Definition of done

1. `UNIT_WIDTH` is gone from `osc.rs`; `inject` stamps `forced_width(cell)`, read **before**
   `set_symbol`, on both the head and the tail cell.
2. `forced_width` reads `CellWidth::cell_width` on the **`Cell`**, not on `cell.symbol()`, so a
   one-cell run (`start == end`) does not re-measure the escape it just prepended.
3. A zero-width symbol maps to `ForcedWidth(1)`, via `NonZeroU16::MIN` — no `unwrap`, no `expect`.
4. The new clause-10 test is present and fails on a revert of (1); the wide-grapheme render's
   per-cell `cell_width()` matches the unlinked render's at every column.
5. `strip_ansi(linked) == plain` byte-for-byte for the wide-grapheme fixture.
6. `CellDiffOption::Skip` is still not used anywhere in the workspace.
7. All nine pre-existing tests in `osc_hyperlinks.rs` still pass unchanged — in particular
   `a_wrapped_path_emits_one_pair_per_row_with_the_same_href` and
   `two_links_in_one_pass_resolve_to_distinct_hrefs`.
8. The `KNOWN LIMITATION` block at `osc_hyperlinks.rs:16-26` is replaced.
9. Workspace `cargo check` and `cargo clippy` stay clean; the suite stays at zero failures.
