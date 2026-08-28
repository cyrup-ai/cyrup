---
stage: qa
status: completed
updated: 2026-08-28 18:40
---

# Alt-screen test parity — one outstanding item

The port, the seam, the prompt-navigation rework and the mapping table are all **complete and
verified**. Do not redo any of it.

Confirmed passing: 26 tests in `src/tests/alt_screen.rs`; the prompt walk asserts real jumps in both
directions plus both no-match legs; `sync_document` is exercised over a populated transcript; the
table's stated totals (16 PORTED / 3 DIVERGENT / 24 N/A) match a recount of its own rows and row 17
cites the test that actually asserts a jump; `scroll_to_row_for_test` is the only accessor added;
`cargo build -p cyrup-tui --all-targets` 0 errors / 0 warnings; `cargo clippy -p cyrup-tui
--all-targets` **0**; `cargo clippy --workspace --all-targets` 0/0; `cargo test --workspace`
8244 passed / 0 failed.

## Two `#[allow(dead_code)]` in `selection.rs` are now false

Both arrived from main's #93, which was correct when it wrote them — nothing called those functions
then. **This branch is what made them false**, and their `reason` prose now tells a reader the
opposite of the truth.

### `cancel` — `crates/cyrup-tui/src/altscreen/selection.rs:341`

> reason = "… `super::scrollbar_drag`'s module doc specifies the call site — one line in §B-3's
> mouse dispatcher, on a `true` from `scrollbar_drag::route` — but wiring it would change what a
> grab does to a live selection, so it is **left to the change that lands §B-8's dispatcher** rather
> than folded into a lint pass."

That change landed. `altscreen/mod.rs:775` calls `selection::cancel` on exactly that `true`, and
`a_scrollbar_grab_cancels_an_in_flight_selection` pins the behaviour.

### `has_selection` — `crates/cyrup-tui/src/altscreen/selection.rs:395`

> reason = "… ADR-0005 §B-11's `/copy` fork is the caller named in the doc above; **that fork is not
> wired yet**, and `bounds` stays private so this is the only way to ask."

It is wired: `altscreen/mod.rs:840` (`AltScreen::selection_text`) calls it, and
`app/execute_misc.rs:805` is the `/copy` fork that consumes it.

### Required

- Delete both `#[allow(dead_code, reason = "…")]` attributes. The functions have production callers;
  the lint will not fire.
- Keep the `///` doc comments above them untouched — those describe the functions and are accurate.
  Only the attributes are stale.
- If any sentence in a surviving doc comment still says the caller is unwired, correct it to name
  the real call site.

The same staleness was already found and removed from `ImageLifecycle::tracked` during the rebase,
once `clear_placements_for_redraw` gave it a production caller (`images.rs:420`). These two were
missed only because the rebase did not conflict in `selection.rs`.

## Acceptance criteria

- [ ] `grep -rn 'allow(' crates/cyrup-tui/src/altscreen/*.rs` returns **nothing** outside doc text
- [ ] `cargo build -p cyrup-tui --all-targets` — 0 errors, **0 warnings** (proves the lint does not
      fire without the suppression)
- [ ] `cargo clippy -p cyrup-tui --all-targets` — **0**, matching `origin/main`
- [ ] `cargo test --workspace --no-fail-fast` — no regression
- [ ] No `///` doc comment in `selection.rs` still claims `cancel` or `has_selection` has no caller

## Constraints

- Attributes only. Do not touch the function bodies, the doc comments' descriptive content, or any
  test.
- Do not delete an unused function to silence a warning: in a parity port an uncalled function means
  an unported caller. That mistake hid a real bug once already (the full-redraw kitty branch). The
  inverse — a stale `allow` insisting a caller does not exist when it does — is the same defect
  pointed the other way, which is what this item fixes.

## Additionally verified on re-review (2026-08-28), no action needed

- **`for_test` never touches the real terminal.** `grep -rn 'enable_raw_mode|disable_raw_mode|is_tty'`
  over `altscreen/*.rs` returns only a doc-comment mention in `terminal.rs:26`. Every byte
  `TerminalSetup::enter` emits goes through the injected sink, so a test cannot switch the
  `cargo test` process to the alternate screen.
- **No cross-test interference from the mouse global.** `MouseSetup::enable` sets the process-wide
  `REPORTING` flag, but its only reader is `map_reader_event` (`mouse.rs:217`), which none of the 26
  tests call — they construct `MouseEvent`s and call `handle_mouse` directly. Worth knowing if
  someone later tests that mapper under `cargo test`'s thread pool.
- **No tautological tests.** All 26 carry assertions, and none asserts only on a value it just
  assigned.

## Note on a stale criterion, for the record

The previous definition of done required clippy to be "8, not increased vs `origin/main`". Main's
#93 has since cleared every finding across all packages, so the correct target is **0**, which this
branch already meets. Not a defect; recorded so it is not re-raised.
