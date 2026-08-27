---
stage: exec
status: done
updated: 2026-08-27 06:42
---

# Close The `&str` Slicing Hole In The No-Panic Policy With `deny(clippy::string_slice)`

> Identified by the `cyrup-tui` hygiene audit (6-dimension fan-out, adversarially verified).
> **Priority:** high · **Effort:** medium
> Augmented 2026-08-27 against `HEAD` = `df64e81` (code identical to `origin/main` = `d2c5b1e`;
> `git diff --stat d2c5b1e..HEAD` touches `.flux/` only). Every line number below was re-read
> from the file on this commit.

## Why the lint

The crate's declared safety posture is the four workspace lints denied at
[`Cargo.toml:96-101`](../../Cargo.toml) (`unwrap_used`, `expect_used`, `panic`,
`indexing_slicing`), inherited by every member through `[lints] workspace = true`
([`crates/cyrup-tui/Cargo.toml:11-12`](../../crates/cyrup-tui/Cargo.toml)), reinforced by
`#![forbid(unsafe_code)]` at [`lib.rs:41`](../../crates/cyrup-tui/src/lib.rs) and a release
profile with `panic = "abort"`.

`clippy::indexing_slicing` only fires on `ty::Slice` / `ty::Array` receivers. `str` / `String`
range indexing is `ty::Str` and is **exempt** — it is tracked by the separate, allow-by-default
restriction lint `clippy::string_slice`. So `cargo clippy -p cyrup-tui --all-targets` passes
clean today while 14 production sites range-slice a `&str`, each of which panics on a
non-char-boundary byte offset, in widgets that consume arbitrary Unicode keystrokes.

All 14 are boundary-safe **today** — the `find`/`rfind`-derived offsets are boundaries by
construction, and the five cursor-driven sites maintain a hand-rolled byte-offset invariant
(`insert` then `cursor += c.len_utf8()`; `cursor -= ch.len_utf8()`). That is unenforced
discipline invisible to every tool. The point of the task is to make it mechanical, not to fix
a live crash.

## Current state — which half landed, which did not

| half | state | evidence |
|---|---|---|
| Remove the `commands.rs` / `transcript/content.rs` slices | **landed** | `grep -nE '\[[^]]*\.\.[^]]*\]' crates/cyrup-tui/src/commands.rs crates/cyrup-tui/src/transcript/content.rs` returns one line — `commands.rs:263`, a **doc comment** quoting the old expression (`` `text[name.len()+1..].trim()` ``) — and no code. The originally-cited `commands.rs:376,379,382` and `transcript/content.rs:66,67` no longer exist. |
| Add `#![deny(clippy::string_slice)]` | **NOT landed** | `grep -n 'string_slice' -r crates/cyrup-tui/` returns nothing at all. [`lib.rs`](../../crates/cyrup-tui/src/lib.rs) carries exactly one inner attribute — `#![forbid(unsafe_code)]` at `:41`. |

So the regression door is still open: nothing stops a new `&str` range slice from landing, and
14 production + 25 test sites are still written in the panicking form.

### Correction to the previous re-verification pass

The earlier section in this file reported the split as **15 production / 24 test = 39**. The
total 39 is right; the split was mis-apportioned. Counted site-by-site from the source on this
commit it is **14 production / 25 test = 39**. `run_loop_draw_coalescing.rs` contributes three
sites (`46`, `50`, `60`), not two. Several test line numbers had also drifted; they are
re-anchored below.

## Production sites — 14, with the concrete rewrite for each

The four cursor families already have a lint-clean sibling in this crate: `.get(range)` +
`.and_then(|s| ...)`, at [`login_dialog.rs:441,451,460,467`](../../crates/cyrup-tui/src/login_dialog.rs),
[`oauth_selector.rs:181`](../../crates/cyrup-tui/src/oauth_selector.rs) and
[`autocomplete.rs:253`](../../crates/cyrup-tui/src/autocomplete.rs) — five worked examples to copy.

### [`text_input.rs`](../../crates/cyrup-tui/src/text_input.rs) — 4 sites

`buffer: String`, `cursor: usize` (documented "always a char boundary", `:35-36`). All four are
`.get(..)` + the `else`/`if let` arm the code already has. This is the `login_dialog.rs:439-469`
shape verbatim — that struct is the same `(buffer, cursor)` pair.

| line | today | rewrite |
|---|---|---|
| `115` (`backspace`) | `let Some(ch) = self.buffer[..self.cursor].chars().next_back() else { return };` | `let Some(ch) = self.buffer.get(..self.cursor).and_then(\|s\| s.chars().next_back()) else { return };` |
| `122` (`delete_forward`) | `let Some(ch) = self.buffer[self.cursor..].chars().next() else { return };` | `let Some(ch) = self.buffer.get(self.cursor..).and_then(\|s\| s.chars().next()) else { return };` |
| `128` (`cursor_left`) | `if let Some(ch) = self.buffer[..self.cursor].chars().next_back() {` | `if let Some(ch) = self.buffer.get(..self.cursor).and_then(\|s\| s.chars().next_back()) {` |
| `134` (`cursor_right`) | `if let Some(ch) = self.buffer[self.cursor..].chars().next() {` | `if let Some(ch) = self.buffer.get(self.cursor..).and_then(\|s\| s.chars().next()) {` |

The `let ... else { return }` / `if let` arms are already present in all four, so the degrade
path is unchanged: a desynced cursor becomes a no-op keystroke instead of an abort.

### [`session_selector.rs:488`](../../crates/cyrup-tui/src/session_selector.rs) — 1 site

```rust
// today (inside `backspace`, guarded by `if self.cursor == 0 { return; }` at :485)
let prev = self.query[..self.cursor].chars().next_back();
// rewrite
let prev = self.query.get(..self.cursor).and_then(|s| s.chars().next_back());
```

The `if let Some(ch) = prev { ... }` block at `:489` already handles `None` by doing nothing.

### [`model_selector.rs:194`](../../crates/cyrup-tui/src/model_selector.rs) — 1 site

```rust
// rewrite — byte-identical to the `oauth_selector.rs:181` form already in the crate
if let Some(ch) = self.query.get(..self.cursor).and_then(|s| s.chars().next_back()) {
```

### [`tree_selector.rs:635`](../../crates/cyrup-tui/src/tree_selector.rs) — 1 site

Already a let-chain; the `.get` folds straight into it and the `edit.cursor > 0` guard stays.

```rust
if edit.cursor > 0
    && let Some(ch) = edit.query.get(..edit.cursor).and_then(|s| s.chars().next_back())
```

### [`keymap.rs:536,538`](../../crates/cyrup-tui/src/keymap.rs) — 2 sites

Both slices are a **prefix test** on the `f1`…`f12` match arm, so `strip_prefix` is the right
tool, not `.get`: it removes both index expressions *and* the hand-written `other.len() >= 2 &&
other.starts_with('f')` guard, which exists only to make the slice safe.

```rust
// today
other if other.len() >= 2
    && other.starts_with('f')
    && other[1..].bytes().all(|b| b.is_ascii_digit()) =>
{
    match other[1..].parse::<u8>() {
        Ok(n @ 1..=12) => code = Some(KeyCode::F(n)),
        _ => return Err(TuiError::KeySpec(s.to_string())),
    }
}

// rewrite
other
    if other
        .strip_prefix('f')
        .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit())) =>
{
    match other.strip_prefix('f').and_then(|d| d.parse::<u8>().ok()) {
        Some(n @ 1..=12) => code = Some(KeyCode::F(n)),
        _ => return Err(TuiError::KeySpec(s.to_string())),
    }
}
```

An `if let` guard (`other if let Some(d) = other.strip_prefix('f') && ...`) would bind `d` once
instead of calling `strip_prefix` twice, but the crate uses **no** `if let` match guards today
(`grep -rnE 'if let .* =>' crates/cyrup-tui/src` is empty), so the `is_some_and` form above is
the one that matches house style. Either is acceptable; do not leave both.

### [`theme.rs:1375,1378,1379`](../../crates/cyrup-tui/src/theme.rs) — 3 sites

`parse_auto_theme_setting` finds the first `/` and slices around it three times.
`str::split_once` is exactly that operation and removes all three at once, with no `Option`
plumbing added — the function already returns `Option`.

```rust
// today
let s = setting?;
let first = s.find('/')?;
if s[first + 1..].contains('/') { return None; }
let light = s[..first].trim();
let dark = s[first + 1..].trim();

// rewrite
let (light, dark) = setting?.split_once('/')?;
// Reject a second slash (Pi: `indexOf("/", slashIndex+1) !== -1`).
if dark.contains('/') { return None; }
let (light, dark) = (light.trim(), dark.trim());
```

Semantics are identical: `split_once` splits on the *first* match, which is what `find` returned,
and the `?` replaces the `find(...)?`.

### [`transcript/tool_result.rs:166,168`](../../crates/cyrup-tui/src/transcript/tool_result.rs) — 2 sites

`idx` comes from `output.rfind("\n\n[")` in the same let-chain, and both halves are wanted.
`str::split_at_checked` yields both in one non-panicking call and folds into the existing chain;
the `else` path is the function's existing `output.to_string()` fallback, so no new degrade path
is introduced and no `unwrap_or(output)`-style silent substitution is needed.

```rust
// today
&& let Some(idx) = output.rfind("\n\n[")
&& output[idx..].contains(path)
{
    return output[..idx].trim_end().to_string();
}

// rewrite
&& let Some(idx) = output.rfind("\n\n[")
&& let Some((head, tail)) = output.split_at_checked(idx)
&& tail.contains(path)
{
    return head.trim_end().to_string();
}
```

(Equivalent two-call form if `split_at_checked` is undesirable:
`&& let Some(head) = output.get(..idx) && let Some(tail) = output.get(idx..)`.)

### Confirmed NOT string slices (do not touch)

`grep` for range-indexing in this crate also hits these; every one is a `Vec`/slice/byte-buffer
receiver, which `clippy::indexing_slicing` (already denied) governs and `string_slice` does not:
`drain.rs:183`, `terminal_query.rs:504`, `escape_reassembly.rs:421,433,571,603,708`,
`extension_editor.rs:641,642`, `session_selector.rs:1395`, and the `Vec<String>` row slices in
`tests/output_pad.rs`, `tests/markdown.rs`, `tests/editor_fidelity.rs`,
`tests/inline_stacking.rs:272`, `tests/dialog_envelope_spacers.rs:95,549`.

## Test sites — 25 across 13 files, handled by allow, not rewrite

Every one is a source-scraping or buffer-inspection guard over an `include_str!` body or a
rendered row, where byte slicing is the point and the offsets come from a `find` on the same
string. Rewriting them would obscure the assertion. The crate convention is already established:
each of these modules opens with

```rust
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
```

**The work is to add `clippy::string_slice` to that existing list — one lint name per file, 13
edits, no source rewrites.** Exact attribute lines to extend (all verified on this commit):

| file (under [`src/tests/`](../../crates/cyrup-tui/src/tests)) | allow attr line | slice sites |
|---|---|---|
| [`run_loop_cancel_bias.rs`](../../crates/cyrup-tui/src/tests/run_loop_cancel_bias.rs) | `23` | 34, 37, 38, 67, 68, 69 |
| [`run_loop_swap_arm_reachable.rs`](../../crates/cyrup-tui/src/tests/run_loop_swap_arm_reachable.rs) | `31` | 42, 43, 44 |
| [`run_loop_input_priority.rs`](../../crates/cyrup-tui/src/tests/run_loop_input_priority.rs) | `25` | 36, 39, 40 |
| [`run_loop_draw_coalescing.rs`](../../crates/cyrup-tui/src/tests/run_loop_draw_coalescing.rs) | `27` | 46, 50, 60 |
| [`render_cache_tick.rs`](../../crates/cyrup-tui/src/tests/render_cache_tick.rs) | `25` | 39, 43 |
| [`thinking.rs`](../../crates/cyrup-tui/src/tests/thinking.rs) | `14` | 157 |
| [`settings_trust_selectors.rs`](../../crates/cyrup-tui/src/tests/settings_trust_selectors.rs) | `8` | 221 |
| [`selector_wiring.rs`](../../crates/cyrup-tui/src/tests/selector_wiring.rs) | `14` | 70 |
| [`scoped_models.rs`](../../crates/cyrup-tui/src/tests/scoped_models.rs) | `7` | 73 |
| [`footer_chrome_fidelity.rs`](../../crates/cyrup-tui/src/tests/footer_chrome_fidelity.rs) | `1` | 952 |
| [`editor.rs`](../../crates/cyrup-tui/src/tests/editor.rs) | `2-7` (multi-line list) | 703 |
| [`dialog_envelope_spacers.rs`](../../crates/cyrup-tui/src/tests/dialog_envelope_spacers.rs) | `34` | 380 |
| [`auth_selector.rs`](../../crates/cyrup-tui/src/tests/auth_selector.rs) | `9` | 227 |

`editor.rs` is the only one whose attribute is already a multi-line list (`:2-7`); add the name
as a sixth entry there rather than collapsing it.

Two nearby allow blocks that must **not** grow a `string_slice` entry:

- [`text_input.rs:237`](../../crates/cyrup-tui/src/text_input.rs) — the inline `#[cfg(test)] mod
  tests` allow (`:236-238`). Its test body has no `&str` slice, so adding the lint there would
  only risk masking the four production fixes at `:115-134` in the same file. Leave it as is.
- Nothing in [`crates/cyrup-tui/tests/`](../../crates/cyrup-tui/tests) — `experimental_marker.rs`
  and `share_viewer_url.rs` contain no range indexing at all, so `--all-targets` needs no change
  there. (Note those are separate crates and would not inherit `lib.rs`'s inner attribute anyway.)

## Where the deny goes — and why not the workspace

**Put it in [`crates/cyrup-tui/src/lib.rs`](../../crates/cyrup-tui/src/lib.rs), immediately below
`#![forbid(unsafe_code)]` at `:41`:**

```rust
#![forbid(unsafe_code)]
// `clippy::indexing_slicing` (workspace-denied) only fires on slice/array receivers; `str` range
// indexing is exempt and panics on a non-char-boundary byte offset. Use `.get(..)`/`split_once`/
// `strip_prefix`/`split_at_checked` instead. Test modules opt out alongside the other four.
#![deny(clippy::string_slice)]
```

Two homes were considered:

1. **Workspace `[lints.clippy]` in [`Cargo.toml:96-101`](../../Cargo.toml)** — rejected. It would
   reach all 21 members at once; the same clippy run shows roughly 80 further sites outside this
   crate (~20 in `cyrup-resources/src/package/git_url.rs` alone), turning a bounded crate-level
   hygiene fix into a workspace-wide migration with no owner. Promotion is a follow-up once other
   crates are clean.
2. **A crate-local `[lints.clippy]` in [`crates/cyrup-tui/Cargo.toml`](../../crates/cyrup-tui/Cargo.toml)**
   — rejected. That manifest is `[lints] workspace = true` (`:11-12`); a crate-local lint table is
   not additive with workspace inheritance, so taking this route means restating all five
   workspace clippy lints in the crate manifest and permanently forking them from the
   single source of truth. The inner attribute in `lib.rs` composes cleanly with the inherited
   table and sits beside the crate's other safety declaration.

## Definition of Done

- [ ] `grep -n 'deny(clippy::string_slice)' crates/cyrup-tui/src/lib.rs` → one hit, adjacent to `#![forbid(unsafe_code)]`
- [ ] `grep -rnE '\[[^]]*\.\.[^]]*\]' crates/cyrup-tui/src --include=*.rs | grep -v '/tests/'` shows no `str`/`String` receiver among the hits — the `Vec`/byte-buffer hits listed above remain, as does the doc-comment hit at `commands.rs:263`
- [ ] `grep -rn 'string_slice' crates/cyrup-tui/src | grep allow` → exactly the 13 test files above; never a production item, and not `text_input.rs:237`
- [ ] `cargo build -p cyrup-tui` → 0 warnings
- [ ] `cargo test -p cyrup-tui` → **1271 passed**, 0 failed (existing tests only; none added)
- [ ] `cargo clippy -p cyrup-tui --all-targets` → the single pre-existing `byte_char_slices` warning at [`escape_reassembly.rs:972`](../../crates/cyrup-tui/src/escape_reassembly.rs) (`for intro in [b']', b'P', b'_']`) and nothing else — zero `string_slice` diagnostics
- [ ] `cargo clippy -p cyrup-tui --all-targets -- -W clippy::string_slice` → no additional diagnostics beyond that baseline (proves the allow blocks cover the test sites rather than the lint simply being off)

## Evidence anchors (all re-read at `df64e81`)

`Cargo.toml:96-101` (workspace clippy lints) · `crates/cyrup-tui/Cargo.toml:11-12` (`[lints]
workspace = true`) · `crates/cyrup-tui/src/lib.rs:41` (`#![forbid(unsafe_code)]`, the only inner
attribute) · production sites `text_input.rs:115,122,128,134`, `session_selector.rs:488`,
`model_selector.rs:194`, `tree_selector.rs:635`, `keymap.rs:536,538`, `theme.rs:1375,1378,1379`,
`transcript/tool_result.rs:166,168` · cursor invariant `text_input.rs:35-36` (doc), `:110-111` (`insert` + `cursor += c.len_utf8()`), `:116` (`cursor - ch.len_utf8()`) ·
lint-clean templates `login_dialog.rs:441,451,460,467`, `oauth_selector.rs:181`,
`autocomplete.rs:253` · clippy baseline `escape_reassembly.rs:972`
