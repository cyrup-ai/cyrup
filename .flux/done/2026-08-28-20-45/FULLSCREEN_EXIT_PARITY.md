---
stage: qa
status: completed
updated: 2026-08-28 22:47
---

# Fullscreen Exit Parity — one cosmetic item

The seam and both guards are **complete and verified**. Do not redo any of it.

Confirmed: `adopt_fullscreen_renderer` is an exact extraction (same three statements, same order, all
comments preserved), so the capture path cannot drift from production. Both guards are non-vacuous by
mutation — suppressing the repaint fails both, removing `draw_fullscreen`'s drain fails both. Build 0
errors / 0 warnings; `cargo clippy --workspace --all-targets` 0; `cargo test --workspace` 8247 passed
/ 0 failed.

## Import grouping in `crates/cyrup-tui/src/tests/fullscreen_scrollback.rs`

```rust
use crate::altscreen::captured_text;
use crate::app::ModeSwitchOptions;
use crate::TuiRenderMode;      // <- crate-root re-export on its own line
use crate::{App, UiTheme};
```

`TuiRenderMode` is a crate-root re-export (`lib.rs`: `pub use altscreen::{.., TuiRenderMode, ..}`).
Every sibling test file collects root re-exports into a single group and keeps only submodule paths on
their own lines — `tests/app_global_actions.rs:21-24` and `tests/compaction_status.rs:33` are the
pattern. Merge it:

```rust
use crate::altscreen::captured_text;
use crate::app::ModeSwitchOptions;
use crate::{App, TuiRenderMode, UiTheme};
```

- [ ] The two `use crate::` root-import lines are one grouped import
- [ ] `cargo build -p cyrup-tui --all-targets` — 0 errors, 0 warnings
- [ ] `cargo clippy --workspace --all-targets` — 0
- [ ] `cargo test -p cyrup-tui --lib fullscreen_scrollback` — 2 passed

## Not defects — recorded so they are not re-raised

- **`rustfmt` disagrees with this file.** It disagrees with 249 files in the crate, including many this
  branch never touched: the installed version wants 2024-edition import ordering and a wrapped
  `#![allow(..)]`. The single-line form matches `tests/alt_screen.rs:94`. Do not reformat to satisfy it
  unless the whole crate is reformatted in its own change.
- **The witness is the captured escape stream, not `App::scrollback_lines`.** Deliberate: that
  accumulator stays empty across an excursion, because the inline commit path is exactly the one
  `draw_fullscreen` skips.
- **The clear-reorder mutation passes, correctly.** `AltScreen::doc` is the renderer's own rendered
  copy taken by `sync_document`, not a borrow of the transcript's document, so moving
  `clear_document` ahead of `stop` cannot lose anything. A guard failing there would assert something
  untrue.
- **The exit call site's `false` is not reachable from a unit test.** It is a literal inside
  `App::run`'s event loop. This is documented in the test's module doc along with what would close it
  (extracting that shutdown tail); it is out of scope here, not an oversight.
