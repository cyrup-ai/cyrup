---
stage: qa
status: completed
updated: 2026-08-28 01:42
---

# Inbound message card — outstanding

> **QA 2026-08-28 01:09.** All three defects are FIXED and verified, and are not restated here:
> the `--resume` drop (`extension_render_message` returns `Rendered`; the replay carries it whole
> through `push_custom_message_rendered` into the `Rendered::Live` arm at
> [`transcript/render.rs:197`](../../crates/cyrup-tui/src/transcript/render.rs) — the same terminal
> arm the live surface uses, so the two now converge); the SGR stride, whose arithmetic is correct in
> every form traced (`38;5;196;1` applies both, `38;2;1;2;3` unchanged, `38;2;1` skips whole, bare
> `38` consumes one, `1;38;5;196;2` applies all three, `38;5;300` fails parse without desyncing);
> and the panic clause, whose containment is real at
> [`facade.rs:1367`](../../crates/cyrup-ext/src/facade.rs). Workspace 7899/7899, intercom 79/79.
>
> One item remains.

## The `"38"` comment misdescribes its own control flow — for the second time

[`cyrup-tui/src/ansi.rs`](../../crates/cyrup-tui/src/ansi.rs), the comment above the `"38"` arm:

> A short or unrecognized tail consumes only the introducer, leaving the rest to be parsed rather
> than skipped whole.

That holds for an unrecognized introducer form (`38;9` falls to `_ => {}` and advances 1) and for a
bare `38` (`codes.get(i + 1)` is `None`, same arm). It is FALSE for a short TRUECOLOR tail:
`ESC[38;2;1m` matches `Some(&"2")`, fails the `r`/`g`/`b` parse, and still runs `i += 4` — skipped
whole, the exact opposite of what the sentence promises.

This is the second wrong description of the same twelve lines. The version this replaced claimed
"A short or non-numeric tail is skipped whole, never half-applied" while the code half-applied by
advancing a fixed four; the replacement over-corrected into claiming the introducer-only behaviour
is universal. Both readings are plausible from the prose and neither matches the code, in a
hand-rolled parser that nothing exercises.

**Required fix.** State the two paths separately, so each matches the arm it describes. The
behaviour itself is correct and must NOT change — only the description:

- a RECOGNISED form consumes its whole tail even when truncated (`38;2;…` advances 4, `38;5;…`
  advances 2), so a malformed colour is skipped entire rather than half-applied;
- an UNRECOGNISED or absent form consumes only the introducer, so whatever follows is still parsed
  as ordinary codes.

## Definition of Done

- The `"38"` comment describes both paths, and each sentence is true of the arm it sits above.
- No behavioural change: `38;5;196;1` still applies indexed foreground AND bold, `38;2;1;2;3` is
  still unchanged, and `38;2;1` is still skipped whole.
- Workspace and `cyrup-it --features it -E 'binary(intercom)'` stay green.
