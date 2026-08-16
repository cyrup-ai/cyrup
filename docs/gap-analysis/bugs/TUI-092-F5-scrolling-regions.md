# TUI-092-F5 — Enable ratatui `scrolling-regions` so commit frames keep the cell diff

> **Part of** [`TUI-092-progressive-lockup.md`](TUI-092-progressive-lockup.md) (the umbrella audit).
> This file is the **only** defect that is a one-line change; it ships first and is fully
> independent of F1–F4, F6–F8.
>
> **Kind** `cyrup-original` · **Severity** high · **Effort** S · **Phase driven** 2 (commit cadence
> during a turn)

## Coordinates with

Nothing. No TUI source changes — the dispatch is compile-time inside ratatui. Land first.

---

## Evidence

[`crates/cyrup-tui/Cargo.toml:50`](../../../crates/cyrup-tui/Cargo.toml#L50) enables only
`unstable-rendered-line-info`. Without the `scrolling-regions` feature,
[`Terminal::insert_before`](../../../tmp/ratatui-core-0.1.2/src/terminal/inline.rs#L109) compiles to
[`insert_before_no_scrolling_regions`](../../../tmp/ratatui-core-0.1.2/src/terminal/inline.rs#L130),
whose last statement is `self.clear()?`
([`inline.rs:212`](../../../tmp/ratatui-core-0.1.2/src/terminal/inline.rs#L212) — *"Clear the
viewport off the screen"*). A ratatui `clear()` invalidates the cell buffers, so the frame after
**every** commit flush repaints the entire viewport instead of cell-diffing — and commits happen on
every finished tool, every finalized message, every turn end (`commit_finished_leading_tools` keeps
them coming mid-turn). With the feature on,
[`insert_before_scrolling_regions`](../../../tmp/ratatui-core-0.1.2/src/terminal/inline.rs#L228)
moves the screen with DECSTBM scroll-region ops and the viewport keeps its diff; the crossterm
backend implements both ops
([`ratatui-crossterm-0.1.2/src/lib.rs:362-383`](../../../tmp/ratatui-crossterm-0.1.2/src/lib.rs#L362)).

**Verified in the tree:** the no-regions `self.clear()?` is at
[`inline.rs:212`](../../../tmp/ratatui-core-0.1.2/src/terminal/inline.rs#L212) (tmux-workaround
comment [`:210`](../../../tmp/ratatui-core-0.1.2/src/terminal/inline.rs#L210)–`211`); the
scrolling-regions path starts at
[`inline.rs:228`](../../../tmp/ratatui-core-0.1.2/src/terminal/inline.rs#L228); the crossterm
`scroll_region_up`/`down` impls at
[`tmp/ratatui-crossterm-0.1.2/src/lib.rs:362`](../../../tmp/ratatui-crossterm-0.1.2/src/lib.rs#L362)–`383`.

**Cost shape.** bytes/frame spike per commit. Every `flush_committed` → `insert_before` ends in a
full viewport repaint instead of a cell diff.

---

## FIX — one line

Enable the feature; no TUI code changes (the dispatch is compile-time inside ratatui):

```toml
ratatui = { version = "0.30.2", features = ["unstable-rendered-line-info", "scrolling-regions"] }
```

DECSTBM + SU/SD are xterm-standard and supported by kitty, ghostty, WezTerm, iTerm2, Alacritty,
Windows Terminal and tmux — the entire terminal matrix this app already targets. This also retires
the *"weird bug with tmux where a full screen clear plus immediate scrolling causes garbage"* hazard
the no-regions path carries its own workaround for
([`inline.rs:210-211`](../../../tmp/ratatui-core-0.1.2/src/terminal/inline.rs#L210)).

---

## Definition of done

* **Commit frames keep the cell diff.** The `scrolling-regions` feature is compiled in
  (`cargo tree -p cyrup-tui -e features | grep scrolling-regions` resolves), so
  `insert_before_scrolling_regions` — not the `clear()` path — serves every flush.
* `cargo build -p cyrup-tui` succeeds with the feature on; no other source file changes.

## Do not touch

The `insert_before` exactly-once discipline in `flush_committed` (the
[`TUI-092-progressive-lockup.md`](TUI-092-progressive-lockup.md) §6 "Do not touch" list covers the
TUI-090 `live_floor` × `insert_before` machinery) — this change is a backend dispatch swap, not a
call-site change.