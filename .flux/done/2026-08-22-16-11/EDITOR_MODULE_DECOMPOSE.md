---
stage: qa
status: completed
updated: 2026-08-22 19:27
---

# Split editor.rs Into An editor/ Package — Rework 2: Six Stranded Production Banners + One Mis-Grouped Helper

> **The defect set is now CLOSED.** An exhaustive comment→annotated-item audit over all 911
> comment lines of the original `editor.rs`, with the `pub(super)` rename noise normalised
> away, finds **exactly 6 broken links — and nothing else**. Two earlier rounds each fixed a
> subset; this scope is provably the remainder. Do not go hunting for more of this class.

## Why the earlier rounds under-scoped, and why this one does not

Round 1 checked *"is a file's last non-blank line a comment?"* — but production files end
with `}` from the `impl InputEditor` wrapper, so a stranded banner sits **second**-to-last.
Only the test files (which end at the last test's `}` with the banner after it) were caught.

The correct check, used here and reproduced in the DoD, is structural rather than positional:
for every comment line in the original, find the next non-blank non-comment line it precedes;
do the same in the new tree; diff the two maps. That is invariant to file boundaries, so it
finds a separated comment wherever it landed. Result: **6**.

## Scope 1 — six stranded production banners

The original carried 9 production section banners. Three are already correctly placed —
`deletion` at [`edit.rs:42`](../../crates/cyrup-tui/src/editor/edit.rs),
`char-jump` at [`motion.rs:238`](../../crates/cyrup-tui/src/editor/motion.rs), and
`autocomplete` at [`history.rs:96`](../../crates/cyrup-tui/src/editor/history.rs) (that last
one is Scope 2's subject). **Six are stranded as the final line inside the wrong file's
`impl InputEditor` block, immediately before its closing `}`:**

| # | stranded at | banner | move to, heading |
|---|---|---|---|
| 1 | `config.rs:313` | `// ---- visual-line map (wrap-aware vertical motion, spec/tui/03 §4) ---…` | `wrap.rs` → `set_view_width` |
| 2 | `wrap.rs:161` | `// ---- large-paste markers (spec/tui/03 §5.5) ---…` | `paste.rs` → `handle_paste` |
| 3 | `undo.rs:75` | `// ---- insertion ---…` | `edit.rs` → `insert_char` |
| 4 | `kill_ring.rs:56` | `// ---- motion ---…` | `motion.rs` → `move_left` |
| 5 | `motion.rs:263` | `// ---- history ---…` | `history.rs` → `push_history` |
| 6 | `completion.rs:148` | `// ---- key handling ---…` | `keys.rs` → `handle_key` |

**Note the stranded line is at the end of the `impl` block, not the end of the file.**
`wrap.rs:161` and `motion.rs:263` are followed by the impl's `}` and then that module's free
functions — `tail -n` on the file will not show them.

### Exact placement

Every destination has the identical head shape: `use super::*;` on line 1, blank, then
`impl InputEditor {` on line 3. Insert so the result reads exactly as the original did
(verified at `editor.rs:598-603`, `:757-762`, `:973-977`, `:1237-1240`, `:1498-1502`,
`:1685-1689`):

```rust
use super::*;

impl InputEditor {
    // ---- <banner, verbatim> ----------------------------------------------------------------

    /// <the destination's existing first doc line — unchanged>
    pub fn <first method>(…) {
```

i.e. banner as the first line inside the block, one blank line, then the existing first item.
Production banners are indented **4 spaces** — unlike the test banners, which are at column 0.
Delete the blank line each banner leaves behind at the end of its source file, so those files
end `}` / `}` with no trailing gap.

Copy each banner **verbatim including its dash padding**. They are alignment-padded to
different widths (the six run 88–92 characters); do not re-pad, re-word or regenerate them.
The safe method is to cut the line, not retype it.

Two destinations end up with two banners, exactly as the original had:
`edit.rs` gains `insertion` at the top and keeps `deletion` at `:42`;
`motion.rs` gains `motion` at the top and keeps `char-jump` at `:238`.

Do **not** invent banners for `config.rs`, `undo.rs`, `kill_ring.rs`, `render.rs` or
`completion.rs` — the original had none before those groups.

## Scope 2 — `lines_as_strings` belongs in `completion.rs`

[`history.rs:96-101`](../../crates/cyrup-tui/src/editor/history.rs) currently holds the
`// ---- autocomplete ----` banner and `lines_as_strings` — inside a module about prompt
history. Its only three consumers are all in `completion.rs`
([`:74`](../../crates/cyrup-tui/src/editor/completion.rs),
[`:114`](../../crates/cyrup-tui/src/editor/completion.rs),
[`:138`](../../crates/cyrup-tui/src/editor/completion.rs)).

In the original this helper **opened** the autocomplete section at `editor.rs:1593-1598`,
immediately above `update_autocomplete` at `:1603`. The split's history span (`1501-1598`)
ended one item too late and dragged both the helper and the next section's banner into
`history.rs`. The banner still correctly heads `lines_as_strings`, so this is a mis-grouping
rather than a broken link — which is why the audit above does not count it.

Move `history.rs:96-101` — banner, blank, doc, and body — into `completion.rs`, inserted
immediately after the **second** `impl InputEditor {` at `completion.rs:61`, above
`update_autocomplete`'s doc comment. The result must read exactly as `editor.rs:1593-1603` did:

```rust
impl InputEditor {
    // ---- autocomplete ------------------------------------------------------------------------

    /// The buffer lines as `String`s, for the autocomplete engine.
    fn lines_as_strings(&self) -> Vec<String> {
        self.lines.iter().map(|l| l.iter().collect()).collect()
    }

    /// Recompute the popup after an edit: …
    pub(super) fn update_autocomplete(&mut self) {
```

**Demote it to `fn`.** It was `fn` in the original (`editor.rs:1596`); the `pub(super)` exists
only for the cross-module call that this move eliminates. Promotion count goes 45 → 44.
`history.rs` must end at its `exit_history` method with no trailing banner.

## Do NOT re-group the modules

A section-level analysis shows three original banner sections whose items now span two files:
`large-paste markers` → `paste.rs` + `undo.rs`; `deletion` → `edit.rs` + `kill_ring.rs`;
`key handling` → five files. **These are not defects and must be left alone.** The original's
9 banners are coarser than the 11-way lifecycle split: undo genuinely is not "large-paste
markers", the kill ring genuinely is not "deletion", and the trailing `key handling` banner
simply ran to end-of-file because no further banner followed it. The finer split is an
improvement over the original's sectioning; only banner *placement* is wrong.

## Definition of done

- [ ] The comment→item audit reports **0** broken links. Reproduce it: for every comment line in `.flux`-side snapshot `old_editor.rs` and in `src/editor/**`, map comment → next non-blank non-comment line (strip a leading `pub(super) ` before comparing), then diff the maps
- [ ] Production banners total 9 and test banners total 10, matching the original; none is the last line inside an `impl` block or at EOF
- [ ] `wrap.rs`, `paste.rs`, `edit.rs`, `motion.rs`, `history.rs`, `keys.rs` each open their `impl InputEditor` block with the banner from the table, byte-identical to the original including dash padding and 4-space indent
- [ ] `lines_as_strings`, its banner and its doc live in `completion.rs` above `update_autocomplete`; it is declared `fn`, not `pub(super) fn`; `history.rs` no longer mentions it
- [ ] The promotion audit shows 44 `pub(super)` items with **all 44 justified** (each referenced from a file other than its declarer) — zero over-promotion
- [ ] `cargo build -p cyrup-tui` 0 warnings · `cargo test -p cyrup-tui` 1270 passed with 44 editor tests · `cargo clippy -p cyrup-tui --all-targets` only `escape_reassembly.rs:972`, 0 findings in `editor/`

## Evidence

Exhaustive audit: 911 original comment lines, 0 missing from the tree, **6** annotating a
different item after normalising the `pub(super)` prefix — the six banners tabled above, each
now followed by `}`. Original banner sites and the items they headed: `:599`→`set_view_width`,
`:758`→`handle_paste`, `:974`→`insert_char`, `:1014`→`backspace` (placed OK), `:1238`→`move_left`,
`:1474`→`jump_to` (placed OK), `:1499`→`push_history`, `:1593`→`lines_as_strings` (Scope 2),
`:1686`→`handle_key`. Current stranded sites: `config.rs:313`, `wrap.rs:161`, `undo.rs:75`,
`kill_ring.rs:56`, `motion.rs:263`, `completion.rs:148`. `lines_as_strings` declared
`history.rs:99`, consumed only at `completion.rs:74,114,138`; original form `fn` at
`editor.rs:1596`. Destination head shape verified identical across all six: `use super::*;` /
blank / `impl InputEditor {` at line 3. Test side already correct: 10 banners, 0 trailing, all
byte-identical to the dedented original.
