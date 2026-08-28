---
stage: done
status: completed
updated: 2026-08-28
---

# Restore The Three Missing `handlePaste` Pre-Processing Steps In The Editor

> Identified by the `cyrup-tui` <-> pi port audit (fan-out + adversarial verification).
> **Priority:** low · **Effort:** small · Area: Editor, input, keys and autocomplete

## Objective

Three concrete paste defects, all in one file. (a) A 2-to-10 line paste over 1000 characters is
labelled `[paste #1 +3 lines]` where pi labels it `[paste #1 1400 chars]`. (b) Pasting a file path
immediately after a word produces `seefile/path` instead of pi's `see file/path`. (c) Inside a tmux
popup running `extended-keys-format csi-u`, a multi-line paste loses its newlines and leaks literal
`[106;5u`-style junk into the prompt — which is exactly the failure the upstream decode step was
added to fix.

## Upstream reference

[`packages/tui/src/components/editor.ts:1168-1230`](../../tmp/pi/packages/tui/src/components/editor.ts)
`handlePaste`, in order:

1. **`:1176-1187` CSI-u decode**, *before* `normalizeText`:
   `pastedText.replace(/\x1b\[(\d+);5u/g, …)` maps codepoint 97..=122 to `cp - 96` and 65..=90 to
   `cp - 64`, leaving anything else as the matched text. The comment states the reason verbatim:
   *"so the per-char filter below preserves newlines instead of stripping ESC and leaking the
   printable tail (e.g. `[106;5u`) into the editor."*
2. `:1189-1195` — `normalizeText` (line endings, tab expansion) then drop every char that is not
   `\n` and has `charCodeAt(0) < 32`.
3. **`:1196-1204` path space-prepend** — when the filtered payload matches `/^[/~.]/` **and** the
   character immediately before the cursor exists and matches `/\w/`, prepend a single space
   *"for better readability"*.
4. `:1206-1226` — large-paste marker, entered on `pastedLines.length > 10 || totalChars > 1000`,
   whose **label** is
   `pastedLines.length > 10 ? "[paste #N +L lines]" : "[paste #N C chars]"` (`:1217-1221`) — the
   threshold in the label is `> 10`, the same constant as the entry gate, **not** `> 1`.

## Current state in cyrup-tui

[`editor/paste.rs`](../../crates/cyrup-tui/src/editor/paste.rs) is the nearest Rust. It is a faithful
port of pi's steps 2 and 4's entry gate, but was written as though `handlePaste` began at
`normalizeText`:

- `sanitize_paste` (`:244-256`) starts at the `\r\n` / `\r` normalize, expands `\t` to four spaces,
  and drops `c.is_control()`. **There is no CSI-u decode stage.**
  [`escape_reassembly.rs:701-712`](../../crates/cyrup-tui/src/escape_reassembly.rs)
  `decode_csi_u_encoded_key_code` decodes CSI-u for **key** events only, and
  [`app/input.rs:101-108`](../../crates/cyrup-tui/src/app/input.rs) hands `InputEvent::Paste(s)`
  straight to `handle_paste`, so the sequence arrives inside the payload untouched.
- **No word-boundary space-prepend** exists in `paste.rs` or `editor/edit.rs`.
- The entry gate at `paste.rs:14` is correct — `if line_count > 10 || char_count > 1000` — but the
  **label** conditional at `:23-27` reads `if line_count > 1`, so the bug surfaces only for a
  2..=10-line paste over 1000 chars. Every existing test uses a single-line 1500-char paste
  (`src/tests/editor.rs:568-604`), which is why the wrong constant survived.
- `line_count` is already computed at `paste.rs:12`, so fix (c) needs no new state.

## Subtasks

1. **Label constant** — [`editor/paste.rs:23`](../../crates/cyrup-tui/src/editor/paste.rs): change
   `if line_count > 1` to `if line_count > 10`, matching `editor.ts:1217`. One-character fix; the
   uncovered case is a 3-line, 1200-char paste, which must now label `chars`.
2. **CSI-u decode** — prepend a decode pass to `sanitize_paste`
   ([`paste.rs:244`](../../crates/cyrup-tui/src/editor/paste.rs)) that rewrites `ESC [ <digits> ; 5 u`
   to the control byte pi produces (97..=122 -> `cp - 96`, 65..=90 -> `cp - 64`, otherwise leave the
   whole match verbatim), **before** the `\r\n` normalize and before the `is_control` filter — the
   ordering is the whole point, since the filter is what would otherwise eat the ESC and leave the
   tail. Reuse the parsing shape in
   [`escape_reassembly.rs`](../../crates/cyrup-tui/src/escape_reassembly.rs) if it fits, but keep this
   a string transform over the payload; do not route pastes through the key decoder.
3. **Space-prepend** — this one needs cursor context, so it belongs in `handle_paste`
   ([`paste.rs:10-39`](../../crates/cyrup-tui/src/editor/paste.rs)), **not** in the pure
   `sanitize_paste`: after sanitizing, when the payload starts with `/`, `~` or `.` and the char
   immediately before the cursor is a word character, prepend one space. It must apply to both
   branches — the marker branch and the verbatim-insert branch — because pi prepends before the
   line/char counts are taken (`editor.ts:1196` precedes `:1206`), which also means the added space
   counts toward the 1000-char gate.

## Acceptance criteria

- [ ] `grep -n 'line_count >' crates/cyrup-tui/src/editor/paste.rs` shows `> 10` in both the entry
      gate and the label conditional
- [ ] A 3-line, 1200-char paste produces `[paste #1 1200 chars]`; an 11-line paste still produces
      `[paste #1 +11 lines]`
- [ ] `sanitize_paste` decodes `\x1b[106;5u` to `\n` (106 - 96 = 10) and `\x1b[65;5u` to `\x01`, and
      leaves a non-matching sequence's text in place
- [ ] The decode runs before the `is_control` filter, so a multi-line CSI-u-encoded paste keeps its
      newlines and no `[106;5u` text reaches the buffer
- [ ] With the cursor after `see`, pasting `/tmp/x` yields `see /tmp/x`; with the cursor after a
      space or at column 0 the payload is inserted with no added space; a payload not starting with
      `/`, `~` or `.` never gains one
- [ ] The prepended space is counted by the 1000-char / 10-line gate (it is added before the counts)
- [ ] `cargo build -p cyrup-tui --all-targets` — 0 errors, 0 warnings
- [ ] `cargo clippy -p cyrup-tui --all-targets` — warning count not increased
- [ ] `cargo test -p cyrup-tui` — `src/tests/editor.rs` still passes unchanged (its cases are all
      single-line, so none of the three fixes should move them)

## Constraints

- No tests are to be written for this task; another team owns the test suite.
- No benchmarks are to be written for this task.
- Workspace lints deny unwrap_used, expect_used, panic and indexing_slicing; cyrup-tui also has
  forbid(unsafe_code) and deny(clippy::string_slice).
- Mechanical fidelity to pi where pi's behaviour is the spec; Rust idiom where it is not.
