---
stage: new
status: todo
updated: 2026-08-16 22:10
---

# CMDHINT_01 — Persistent command-token highlighting + argument-hint placeholder in the editor

## STATUS OF THIS FILE

**This is a freshly authored, cyrup-original task**, written in direct response to a user report
during this session, not a port of anything upstream. I checked `pi/packages/tui/src/components/editor.ts`
and `pi/packages/coding-agent/src/modes/interactive/components/custom-editor.ts` — pi does
**neither** half of what this task adds: it never colors a recognized `/command` token in the raw
input line, and it never shows placeholder/ghost argument text after one. There is no `pi/` file
to cite for the new behavior, and none should be invented. Every citation below to an existing
`.rs` file is real and re-checked at HEAD; anything without one is new design, stated as such.

Do not confuse this with the `spec/flux/` series or `spec/flux.md` — this is a `cyrup-tui`
editor-component fix that happens to have been *discovered* by exercising `/flux/aug`, but it
applies identically to every slash command from every source (builtin, prompt template, extension,
skill).

## THE BUG

Reported directly: type `/flux/aug` and cyrup shows a helpful autocomplete row —
`todo_file | number_of_agents | additional_instructions — Augment task(s) with research - single
file or N in parallel`. Type a single space to start supplying the argument, and **every trace of
that help vanishes** — no hint, no description, no visual confirmation the command even resolved.
This is backwards: the moment you've typed the command and are about to supply its argument is
*exactly* the moment you need the hint, and that's the one moment cyrup shows nothing.

Root cause, precisely:

- [`slash_context`](../../crates/cyrup-tui/src/autocomplete.rs) (`autocomplete.rs:139-141`) guards
  on `!before.contains(char::is_whitespace)` — the popup context exists **only** while the token
  left of the cursor has no space in it. The instant a space is typed, this returns `None`.
- [`InputEditor::update_autocomplete`](../../crates/cyrup-tui/src/editor.rs) (`editor.rs:1522-1541`)
  reacts to that `None` by closing the popup outright: `_ => self.autocomplete = None` (`:1540`).
- The composed hint text — `format!("{hint} — {}", cmd.description)` (`autocomplete.rs:155-158`)
  — lives **only** inside that now-discarded `Autocomplete`/`SelectList`
  (`autocomplete.rs:162-163`). Nothing persists any part of it into editor state once the popup
  context ends. There is no fallback, no residual indicator — the screen returns to exactly what
  it would show for an empty buffer.
- [`InputEditor::render`](../../crates/cyrup-tui/src/editor.rs) (`editor.rs:2392-2531`) applies
  exactly one content-independent style choice to the whole input — the top/bottom rule color
  (`rule_style`, bash-mode / thinking-level / muted-border, `:2400-2409`). The per-character span
  loop (`:2471-2497`) only ever distinguishes "the cursor cell" from "everything else"
  (`base` vs `cursor_style`); it never inspects buffer content to decide whether the leading token
  names a real command. There is no styled span anywhere in this file tied to command recognition.

Net effect: help is available for as long as you're *deciding what to type*, and disappears the
moment you've decided and need to know what comes next.

## THE FIX

Two additions to `crates/cyrup-tui/src/editor.rs`, no upstream citation, both reusing existing
theme/registry surface (no new theme keys, no registry changes):

1. **Persistent command-token highlight.** While the leading token (from `/` to the first
   whitespace, or to end-of-line if none yet) is a valid **prefix** of at least one registered
   command name, render that whole typed-so-far token in [`UiTheme::accent_style`]
   (`crates/cyrup-tui/src/theme.rs:312-314`) instead of the base style. This starts the moment
   `/flux` matches (a prefix of `flux/aug`, `flux/status`, …) and keeps re-evaluating on every
   keystroke as you extend it to `/flux/aug`. Once the token is followed by a space **and** it is
   an *exact* match against a registered command name, the highlight **freezes** on that
   `/name` span for the rest of the edit — it no longer depends on the popup or on "still
   filtering" state, only on the fixed byte range and the one-time exact-match check.
2. **Argument-hint ghost text.** The instant that exact-match freeze happens, if the command has
   a non-`None` `argument_hint` and the argument zone is still completely empty, render the hint
   string in [`UiTheme::dim_style`](../../crates/cyrup-tui/src/theme.rs:339) immediately after the
   space, in the same position a shell/editor placeholder occupies. It disappears the instant any
   real character is typed there — standard placeholder semantics, not a fixed label.

## VERIFIED MECHANICS

### Fact 1 — the registry already exposes what's needed, unchanged

[`CommandRegistry::commands()`](../../crates/cyrup-tui/src/commands.rs) (`commands.rs:248-250`)
already holds every autocomplete-visible command — builtin, prompt template, extension, skill — in
one flat `&[SlashCommand]`, and [`CommandRegistry::get(name)`](../../crates/cyrup-tui/src/commands.rs)
(`:252-254`) is an exact bare-name lookup. Both are already used by `slash_context` /
`InputEditor::registry`. **No registry change is needed** — this task is entirely `editor.rs` +
maybe a small new free function in `autocomplete.rs` alongside `slash_context`.

### Fact 2 — prefix match, not fuzzy match, for the highlight decision

`slash_context`'s popup uses [`fuzzy::filter`](../../crates/cyrup-tui/src/fuzzy.rs) (`fuzzy.rs:143`),
which is intentionally lenient (non-contiguous character matches) so the *suggestion list* stays
useful. The highlight this task adds is a different kind of signal — "the text you've actually
typed is a real, honest prefix of a real command" — so it must use a plain
`name.starts_with(query)` check, not the fuzzy scorer. Using fuzzy matching here would highlight
something like `/fx` (which fuzzy-matches `flux/…`) as if the user had typed a real path segment,
which is not true and would be a worse signal than no highlight at all. Keep the two matchers
separate; do not share the fuzzy call between the popup and this feature.

### Fact 3 — `argument_hint` is one opaque string, never split

[`SlashCommand.argument_hint`](../../crates/cyrup-tui/src/commands.rs) (`commands.rs:50`) has two
observed shapes at HEAD, neither positional:

- Builtins use an angle-bracket single placeholder: `arg_cmd("model", …, "<provider/model>")`,
  `arg_cmd("login", …, "<provider>")` (`commands.rs:70`, `:86`).
- Flux's frontmatter-sourced hints use `|` to separate **alternative forms of the single
  `$ARGUMENTS` blob**, not sequential positional slots — e.g.
  `argument-hint: todo_file | number_of_agents | additional_instructions`
  (`crates/cyrup-flux/resources/prompts/flux/aug.md:2`). Confirmed against the template body:
  `aug.md`'s STEP 1 detection logic branches on the **entire** `$ARGUMENTS` string being empty,
  `all`, a pure integer, or a filename (`aug.md:32-52`) — there is no positional splitting
  anywhere in the template, and cyrup's own prompt-template engine's real positional substitution
  (`$1`, `$2`, `$@`, ported from pi's `prompt-templates.ts` — see
  [`crates/cyrup-resources/src/prompt.rs:1-10`](../../crates/cyrup-resources/src/prompt.rs)) is
  simply not used by any flux template; they all consume `$ARGUMENTS` as one string.

**Do not tokenize `argument_hint` on whitespace.** Doing so would fragment
`todo_file | number_of_agents | additional_instructions` into five garbage pieces around the `|`s.
Treat and render it as a single opaque string, exactly as the existing popup composition already
does (`autocomplete.rs:155-158`).

### Fact 4 — scope: logical line 0 only

A slash command is only ever recognized as one when it is the leading text of the whole submitted
buffer ([`CommandRegistry::dispatch`](../../crates/cyrup-tui/src/commands.rs) `commands.rs:267-278`
trims and checks `strip_prefix('/')` on the **whole** trimmed text). In ordinary use a slash
command and its arguments are typed on one line; a soft-newline (`\` + Enter,
`editor.rs:1865-1867`) continuing onto further lines while still supplying arguments to the same
command is a real but rare case. Scope this task to **logical line 0**:

- The command-token highlight only ever looks at `lines[0]`.
- The ghost placeholder only renders when the **entire buffer** past the command token + space is
  empty — i.e. `lines[0]`'s own remainder after the space is empty **and** every line after line 0
  (if any) is empty too. The moment there is a character anywhere past that point, treat the
  argument zone as non-empty and suppress the ghost, even though it visually "belongs" at the end
  of line 0.
- If `lines[0]` does not start with `/`, neither feature does anything — unchanged from today.

### Fact 5 — where to hook the computation

[`InputEditor::update_autocomplete`](../../crates/cyrup-tui/src/editor.rs) (`editor.rs:1522-1541`)
already runs on every edit and already has `self.lines_as_strings()` to hand. Add a sibling
private method, e.g. `update_command_highlight(&mut self)`, called from the same edit paths
`update_autocomplete` is (so the two stay in sync — both derive from the same buffer state), that
computes and stores a small piece of state on `InputEditor`, e.g.:

```rust
/// Persistent command-token highlight + argument-hint ghost, independent of the autocomplete
/// popup's open/closed state (CMDHINT_01 — cyrup-original, no pi citation).
struct CommandHighlight {
    /// Char range within `lines[0]` to render in `theme.accent_style()`.
    token_range: std::ops::Range<usize>,
    /// The hint to show as ghost text immediately after the command token + space, when the
    /// buffer has nothing past that point yet. `None` when there's no exact match, no hint, or
    /// the argument zone is already non-empty.
    ghost: Option<String>,
}
```

stored as `Option<CommandHighlight>` alongside the existing `autocomplete: Option<Autocomplete>`
field. Recompute it every time `update_autocomplete` runs (same triggers: any text edit, cursor
move that could change line 0, `/reload` rebuilding the registry). It must NOT be cleared when the
*popup* closes — that's the entire point of this task — so keep its lifecycle separate from
`self.autocomplete`.

### Fact 6 — theme surface already sufficient

[`UiTheme::accent_style`](../../crates/cyrup-tui/src/theme.rs) (`theme.rs:312-314`, "Accent style
(assistant text, focus, emphasis)") and [`UiTheme::dim_style`](../../crates/cyrup-tui/src/theme.rs)
(`theme.rs:328-339`, "Secondary/hint chrome — Pi's `dim` token") are both semantically exactly
right and already used elsewhere in this crate. **No new theme method, no new palette key.**

### Fact 7 — rendering: span-zone splitting has to compose with the existing cursor overlay

The current per-visual-line loop (`editor.rs:2471-2497`) builds spans in one of two shapes:

- the cursor's own visual line: `[before-cursor][cursor grapheme][after-cursor]`, 2-3 spans;
- every other visual line: one plain span for the whole segment.

This task adds a THIRD axis (style zones from `CommandHighlight::token_range`) that must compose
with the cursor split, not replace it — a visual line can contain part of the highlighted token
*and* the cursor. Restructure the span builder to:

1. Compute the segment's style zones first: `[0, token_end)` in `accent_style()` intersected with
   this visual line's `[vl.start, vl.start+vl.len)` window (only ever non-trivial on the visual
   line(s) covering the start of `lines[0]`; empty on every other line), the rest in `base`.
2. Then, if this is the cursor's visual line, split whichever zone(s) the cursor column falls
   inside using the existing grapheme-aware cursor logic (`:2483-2494`) — the cursor cell always
   wins visually over the zone style, same rule as today's `cursor_style` overriding `base`.
3. Ghost text (`CommandHighlight.ghost`, when `Some`) is appended as one extra `dim_style()` span
   at the very end of the buffer's **last** visual line, after the real content and after the
   end-of-line caret cell — it is never part of the cursor-split logic, since it is not real
   buffer content and the cursor can never sit inside it.

A short helper function (e.g. `fn style_zones(seg_len: usize, highlight: Option<Range<usize>>) ->
Vec<(Range<usize>, Style)>`) that both the cursor and non-cursor branches call is the cleanest way
to avoid duplicating the intersection math.

### Fact 8 — the `dispatch_names` gate does NOT need to change

`CommandRegistry::match_command`/`dispatch` (`commands.rs:267-311`) is unaffected — dynamic
commands like `flux/aug` already dispatch correctly via the session's own `split on first space`
routing (documented at `crates/cyrup-flux`'s FLUX_07 review: `session.rs:1041-1042`). This task is
purely a **rendering** change; it does not touch what gets executed, only what gets shown while
typing it.

## SUBTASKS

### SUBTASK 1: `autocomplete.rs` — the prefix-match + exact-match helpers

Add two small, pure, unit-testable functions near `slash_context`:

```rust
/// Whether `query` (text after the leading `/`, no leading-slash) is a real PREFIX of at least
/// one registered command name — plain `starts_with`, deliberately NOT the fuzzy matcher
/// `slash_context` uses for suggestions (CMDHINT_01 Fact 2).
pub fn is_command_prefix(registry: &CommandRegistry, query: &str) -> bool {
    !query.is_empty() && registry.commands().iter().any(|c| c.name.as_ref().starts_with(query))
}
```

(`query.is_empty()` guarded false so a bare `/` does not highlight — nothing has been typed yet to
confirm.)

The exact-match half is just `registry.get(name)` — no new helper needed there.

### SUBTASK 2: `editor.rs` — `CommandHighlight` state + `update_command_highlight`

- Add the `CommandHighlight` struct (Fact 5) and an `Option<CommandHighlight>` field on
  `InputEditor`, initialized `None`.
- Implement `update_command_highlight(&mut self)`:
  1. `lines[0]` must start with `/`, else `self.command_highlight = None; return;`.
  2. Find `sp` = byte index of the first whitespace in `lines[0]` chars, if any.
  3. `head` = `lines[0][1..sp.unwrap_or(len)]` (the query, no leading slash).
  4. If `sp.is_none()`: `token_range = 0..(1+head.len())` when
     `is_command_prefix(&self.registry, head)`, else `None`.
  5. If `sp.is_some()`: check `self.registry.get(head)`. If `Some(cmd)`:
     `token_range = 0..sp` (frozen, includes the leading `/`); `ghost` = `cmd.argument_hint`
     cloned as `String`, but only when the WHOLE-BUFFER remainder past `sp` is empty (Fact 4) —
     else `ghost = None`, but `token_range` still applies (the highlight persists independently of
     whether the ghost is showing). If `self.registry.get(head)` is `None`: `self.command_highlight
     = None` (an unregistered "directory-looking" token like `/flux ` with nothing after it, or a
     genuinely unknown command, gets no highlight and no ghost — honest, matches today's silent
     fallback to a literal prompt).
- Call `self.update_command_highlight()` from every site that currently calls
  `self.update_autocomplete()` (edits, cursor moves that can change line 0, registry reload via
  `set_registry`) — same triggers, independent storage.

### SUBTASK 3: `editor.rs::render` — span-zone builder (Fact 7)

- Factor the zone/cursor/ghost composition into the helper described in Fact 7.
- Verify manually (or with a unit test, SUBTASK 4) that a highlighted zone spanning a
  visual-line-wrap boundary (a very long registered command name, or a narrow terminal) still
  renders correctly on both visual lines it touches — this is the one genuinely new geometry case
  this task introduces; today's code never needs to intersect a style zone against a wrapped
  visual-line window.

### SUBTASK 4: Tests

Despite the flux series' "no tests" convention, this is a **new, previously-untested rendering
path** in a crate that already has 36 files under `crates/cyrup-tui/tests/` — add unit coverage
under `crates/cyrup-tui/src/tests/editor.rs` (or wherever the existing editor render/state tests
live) for:

- `/flux` (partial, no space) → `command_highlight.token_range == 0..5`, no ghost.
- `/flux/aug` (complete, no space yet) → highlighted range covers the whole typed token, no ghost.
- `/flux/aug ` (space, nothing after) → `token_range == 0.."/flux/aug".len()`, `ghost ==
  Some("todo_file | number_of_agents | additional_instructions")` (or whatever
  `flux/aug`'s registered hint is at test time — read it from the fixture registry, don't hardcode
  the string twice).
- `/flux/aug NOTIFS` (space + real argument) → `token_range` unchanged (still highlighted),
  `ghost == None`.
- `/nonsense` and `/nonsense ` → no highlight, no ghost, at every stage (regression guard: an
  unknown command must render exactly as it does today).
- `/model` (a builtin with `<provider/model>` hint) → same behavior via the builtin path, proving
  this isn't flux-specific.
- A command name long enough to wrap across two visual lines in a narrow test terminal width →
  the highlight zone renders correctly split across both `Line`s (Fact 7 / SUBTASK 3's geometry
  case).

### SUBTASK 5: Build, lint, manual check

```bash
cargo build -p cyrup-tui && cargo build -p cyrup
cargo clippy --workspace --all-targets --features test-fixtures; echo "exit=$?"   # MUST be 0
cargo install --path crates/cyrup --force
```

In the TUI:

- Type `/flux` — it highlights in the accent color immediately (before any full command exists).
- Continue to `/flux/aug` — the highlighted span grows with each character; the autocomplete popup
  still behaves exactly as before (this task must not change popup behavior, only add a second,
  independent visual layer).
- Type a space — the popup closes (unchanged), but `/flux/aug` **stays** highlighted, and the dim
  placeholder `todo_file | number_of_agents | additional_instructions` appears right after the
  space.
- Start typing an argument, e.g. `NOTI` — the ghost disappears the instant the first character
  lands; `/flux/aug` remains highlighted through the rest of editing and through submission.
- Backspace the argument back to empty — the ghost reappears (this is live-recomputed state, not
  a one-shot "already dismissed" flag).
- Try an unregistered command, e.g. `/bogus thing` — no highlight at any point, no ghost; behavior
  identical to before this task.
- Try a builtin, e.g. `/model` then a space — `<provider/model>` ghost appears; confirms the
  feature is registry-wide, not flux-specific.

## DEFINITION OF DONE

- [ ] Typing a valid command-name prefix (e.g. `/flux`) highlights it immediately, before the
      command name is complete.
- [ ] The highlight persists, unmodified in mechanism, as the token is completed to an exact match
      (e.g. `/flux/aug`).
- [ ] After a space following an exact match, the highlight **freezes** on the `/name` span and
      survives the popup closing — verified by continuing to type an argument and confirming the
      span stays styled.
- [ ] The argument-hint ghost text appears immediately after the space when the argument zone is
      completely empty (buffer-wide, not just same-line), in `dim_style()`, using the command's
      unmodified, unsplit `argument_hint` string.
- [ ] The ghost disappears the instant any real character exists anywhere past that point, and
      reappears if the buffer is edited back to empty there.
- [ ] An unrecognized command (no prefix match at any point) shows neither highlight nor ghost —
      byte-for-byte the same rendering as before this task.
- [ ] The feature works identically for builtin, prompt-template, extension, and skill commands
      (registry-wide, not flux-specific) — demonstrated with at least one builtin (`/model` or
      `/login`) in addition to a flux command.
- [ ] A style zone that spans a visual-line wrap boundary renders correctly on both wrapped lines.
- [ ] `cargo build -p cyrup-tui`, `cargo build -p cyrup` succeed;
      `cargo clippy --workspace --all-targets --features test-fixtures` exits 0.
- [ ] New unit tests (SUBTASK 4) exist and pass; existing autocomplete/editor tests are unaffected
      (this task adds a parallel state machine, not a replacement of the popup's).
