---
stage: new
status: done
updated: 2026-08-22 18:31
---

# Collapse The Three Divergent str_width/truncate_to_width Copies Onto One Grapheme-Atomic Truncator

> Identified by the `cyrup-tui` hygiene audit (6-dimension fan-out, adversarially verified).
> **Priority:** high · **Effort:** medium

## Description

The crate asserts a single-truncator invariant in its own docs and then violates it in two modules with char-based copies that can cut a grapheme cluster in half.

**Canon** lives in a leaf module: `crates/cyrup-tui/src/settings_selector.rs:31` (`str_width`), `:46` (`truncate_to_width` — walks `text.graphemes(true)`, has the `ew >= max` ellipsis-clip arm), `:564` (`truncate_line_to_width`, grapheme-atomic over styled spans). Five siblings already import it: `pending_messages.rs:51`, `select_list.rs:24`, `user_message_selector.rs:31`, `config_selector.rs:32`, `oauth_selector.rs:35`. `select_list.rs:355-363` documents its local `truncate` as delegating to "the crate's one grapheme-atomic truncator ... so 👨‍👩‍👧 is either present or absent, never reduced to its leading 👨", and `select_list.rs:12-18` calls the char-based form "the char-vs-grapheme defect this crate has now carried in eight separate measurements".

**Two copies violate it.** `session_selector.rs:1076` `truncate_to_width` iterates `for ch in s.chars()` at `:1085-1094` and has no `ew >= max` arm; `session_selector.rs:1102` `truncate_spans_to_width` is built on it. `status.rs:600` `truncate_to_width` delegates to `:612` `truncate_parts`, also `for ch in s.chars()`. Both are reachable on user data: `session_selector.rs:603` truncates a session's own message text, and `:540`, `:658`, `:716` truncate the empty-state message, the scroll row and the header title. A ZWJ family emoji there loses its trailing joiner instead of being dropped whole.

**The work.** Create `crates/cyrup-tui/src/text_width.rs` (a new file, declared in `lib.rs` beside the other `mod` lines at `:37-83`) and move `str_width`, `spans_width`, `truncate_to_width` and `truncate_line_to_width` there from `settings_selector.rs`, carrying their doc comments verbatim — including the `settings_selector.rs:43-45` note explaining why the canon originally lived in a leaf module, reworded to record the move. Do NOT put them in `selector.rs`: that file is being restructured by SELECTOR_MODULE_AND_SHARED_CHROME and a new top-level module avoids the collision. Re-point the five existing importers, then delete `session_selector.rs:1065-1140` and `status.rs:588-625` and have those two modules import from `crate::text_width`.

Two divergences must be preserved deliberately, not silently unified: (a) `session_selector.rs:1102`'s span truncator styles the ellipsis with the last kept span's style while `settings_selector.rs:591` pushes a bare `Span::raw` — make that an explicit parameter or document the chosen behaviour at the call site; (b) `status.rs:612` `truncate_parts` returns `(body, was_truncated)` because `footer.ts:240` colours the ellipsis separately — keep that as the split primitive in the shared module and define `truncate_to_width` on top of it. `markdown.rs:1544`'s local `spans_width` is byte-identical to the shared one and should also be re-pointed while the file is open.

Nothing in `src/tests/` currently pins the char-based behaviour, so no existing assertion should need editing; `src/tests/chrome.rs:151-158` and the editor tests already lock grapheme atomicity elsewhere. Add one new test driving a ZWJ family emoji (`👨‍👩‍👧`) through the `session_selector` row-truncation path and asserting the cluster is absent rather than partially present.

## Acceptance Criteria

- [ ] `grep -rn 'fn truncate_to_width' crates/cyrup-tui/src/` returns exactly one definition, in `src/text_width.rs`; same for `fn str_width` and `fn truncate_line_to_width`
- [ ] `grep -rn 'for ch in s.chars()' crates/cyrup-tui/src/session_selector.rs crates/cyrup-tui/src/status.rs` returns no hits inside a truncation helper
- [ ] A new test truncating a string containing `👨‍👩‍👧` through the session-selector row path asserts the output contains either the whole cluster or none of it (no bare ZWJ, no orphaned component)
- [ ] `status.rs`'s `(body, was_truncated)` split primitive still exists and the footer's separately-coloured ellipsis renders unchanged (existing footer tests pass)
- [ ] `cargo test -p cyrup-tui` passes (1270 tests + the new one), `cargo build -p cyrup-tui` emits 0 warnings, `cargo clippy -p cyrup-tui --all-targets` shows only escape_reassembly.rs:972

## Evidence

crates/cyrup-tui/src/settings_selector.rs:28-60 (canon str_width :31, truncate_to_width :46 walking graphemes(true), home-note :43-45), :564, :591; crates/cyrup-tui/src/session_selector.rs:1063-1094 (char loop :1085-1094), :1102, :540, :603, :658, :716; crates/cyrup-tui/src/status.rs:588,600,612-625; crates/cyrup-tui/src/select_list.rs:12-18,24,355-363; importers pending_messages.rs:51, user_message_selector.rs:31, config_selector.rs:32, oauth_selector.rs:35; crates/cyrup-tui/src/markdown.rs:1544
