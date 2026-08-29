---
stage: new
status: done
updated: 2026-08-29 16:16
---

# `hideThinkingBlock` Must Hide The Block Completely, Not Show A `Thinking ...` Placeholder

## Description

What `hideThinkingBlock` should do is **hide the thinking block completely in the conversation**,
not show `Thinking ...` in a visible block with no thinking ever showing. The previous change
over-reacted and basically made the setting a NOOP, which is wrong, rather than fixing the UI to
actually not show the block — which is the intended functionality and how it works in pi.

## The correction, stated plainly

The `Thinking...` placeholder **is the bug**. When `hideThinkingBlock` is on, a reasoning run
should contribute **nothing** to the transcript — no label, no styled line, and no spacer that
implies a block was there. The conversation should read as if the turn had no reasoning section at
all.

## What is actually in the tree right now

Do not start by re-deriving this; start by reading `crates/cyrup-tui/src/transcript/message.rs`.

- `thinking_lines(text, hidden, width, theme, label)` returns
  `vec![Line::styled(label, style)]` when `hidden` — a single `HIDDEN_THINKING_LABEL`
  (`"Thinking..."`) line. **This is the line to remove.**
- Two callers render it: the committed path in `transcript/render.rs` (`Entry::Thinking`) and the
  live path in `transcript/cache.rs`. Each also pushes a **leading blank line** and a padding pass
  around it — those must go with the label, or hiding leaves a bare gap where the block was.
- `Entry::Thinking { text, hidden }` freezes the choice at commit time. Committed entries that
  were frozen `hidden` currently exist to render the label; decide whether such an entry should be
  skipped at render time or never pushed at all (see the open question below).

**The last change did NOT make the setting inert** — it left the label behaviour untouched and added
three indicators (`/thinking` picker warning, `(hidden)` on the `/settings` row, `(hidden)`
in the footer right cluster). The objection to that framing is the point of this task: those
indicators were compensating for the UI bug instead of fixing it. See the open question.

## A conflict the implementer MUST resolve before writing code

The task states this is how pi works. **The vendored pi checkout at `tmp/pi` contradicts that**, at
`packages/coding-agent/src/modes/interactive/components/assistant-message.ts:139-143`:

```js
if (this.hideThinkingBlock) {
    // Show one static label for each run of thinking blocks when hidden.
    this.contentContainer.addChild(
        new Text(theme.italic(theme.fg("thinkingText", this.hiddenThinkingLabel)), this.outputPad, 0),
    );
}
```

That is pi's own comment, at the pinned revision. Two readings, and the implementer must establish
which before touching code:

1. **pi changed upstream.** `tmp/pi` is pinned; `docs/PARITY-PLAN.md` records pi HEAD as ~117
   commits past the ported baseline. If a later pi removed the label, this is a straight
   **upstream-drift port** — find the commit, cite it, and port it.
2. **pi still renders the label and cyrup is diverging deliberately.** Then this is a
   **`[CYRUP-DELTA]`**, and it must be recorded as one in the code the way every other intentional
   divergence in this crate is, naming what upstream does and why cyrup does not.

Either answer is fine and the work is nearly identical; what is not fine is landing it without
saying which. Do not silently rewrite the existing citations to claim upstream agrees.

## Acceptance criteria

- [ ] With `hideThinkingBlock: true`, a reasoning run contributes **zero lines** to the transcript
      — no label, no blank line, no pad — on **both** the committed path (`transcript/render.rs`)
      and the live streaming path (`transcript/cache.rs`).
- [ ] The surrounding vertical rhythm stays correct: an assistant turn with hidden reasoning followed
      by answer text must not gain or lose a blank line relative to a turn that had no reasoning at
      all. This is what the existing rhythm/spacer tests guard — they will need review, not blanket
      updating.
- [ ] With `hideThinkingBlock: false`, rendering is byte-identical to today.
- [ ] Reading path 1 or 2 above is resolved, in writing, with the upstream citation that settles it.
- [ ] `cargo build --workspace --all-targets` and `cargo clippy --workspace --all-targets` clean;
      the suite green.

## Open question for the user — do the three indicators stay?

They were added on the premise that a *silently suppressed* block is confusing. If the block
disappears completely, that premise gets **stronger**, not weaker: a user running `max` thinking
would otherwise see no trace of reasoning anywhere and no reason why.

- The **footer** `• max (hidden)` is the cheapest standing answer to "where did my thinking go" and
  is the one worth keeping.
- The **`/thinking` picker warning** is the most intrusive and the most arguable.
- The **`/settings` row** marker is redundant once the toggle sits two rows away.

**Do not remove or keep these on the implementer's own judgement — put it to the user.**

## Out of scope

- `hiddenThinkingLabel` / `setHiddenThinkingLabel` (the extension override,
  `UiEffect::SetHiddenThinkingLabel`) — if the label never renders, that API's fate is a separate
  decision, not this task's.
- `Entry::Thinking`'s commit-time freeze generally (`TUI-N06` owns it).
- `Ctrl+T`'s chord, and the plain-Space `/settings` cycle question.
