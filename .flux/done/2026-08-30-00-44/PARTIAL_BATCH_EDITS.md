---
stage: qa
status: completed
type: implementation
updated: 2026-08-30 09:52
---

# Partial Batch Application For The Edit Tool — completed

`edit` was all-or-nothing: one `oldText` that failed to match discarded every other edit in the
same call, even when the rest matched cleanly and uniquely. This was symptom (a) of the original
`EDIT_TOOL` write-up and the only one PR #109 did not address, because it changes the tool's
*contract* rather than its matching.

pi has no such behaviour to port — `edit.ts:369-374` throws on the first failing edit — so every
decision here is a `[CYRUP-DELTA]` with a rationale comment naming what pi does instead. The
precedent is aider, which applies what matches and reports the rest in both engines
(`editblock_coder.py:41-43`, `:120-122`).

## What shipped

- **`edit_diff.rs`** — `AppliedEdits` gained `applied: Vec<usize>` and `failed: Vec<(usize,
  String)>`; the batch loop pushes failures instead of returning on the first one; an overlapping
  pair drops **both** members; an empty `oldText` fails only itself; nothing-matched still writes
  nothing. `join_failures` collapses the one message shared by construction (overlap names both
  indices and is recorded against each). `EditDiffPreview` gained `unapplied`.
- **`edit.rs`** — writes the survivors, then returns `Err` naming both the failures and the applied
  indices via `partial_batch_message`, so a model that skims a success line cannot conclude every
  edit landed.
- **`event_extract.rs`** — the TUI preview shows the survivors' diff plus one line per edit that
  will not land, instead of discarding everything but the diff.

## Invariants worth keeping

- **Offsets.** Every edit matches the *same* original buffer and `apply_replacements` splices in
  reverse, so survivors' offsets were never relative to each other. Never re-match survivors
  against the partially-applied buffer.
- **Disjointness.** Comparing only *adjacent* pairs after the sort is sufficient: if survivors
  `P < Q` overlap then `P.end > Q.start >= R.start` for `P`'s immediate successor `R`, so `P` was
  already collided. Proved, and checked exhaustively over 346,200 interval sets.
- **`applied` is ascending by edit index**, not match position — `sort_unstable` is the only thing
  making that true, and `matched` is sorted by position a few lines above.

## Accepted, deliberate

- **`ToolError` carries only a `String`**, so the diff of what did land cannot reach the TUI on the
  partial path. The preview is where the diff is shown. Do not widen it.
- **`used_fuzzy` is decided over all edits before any failure is known.** Only a tier-2 match
  dropped for duplicate-count or overlap can flip it — never a not-found. Recomputing over the
  survivors needs a re-match whose termination is not obvious.
- **`partial_batch_message` takes two slices, not `&AppliedEdits`.** `execute` moves `new_content`
  out one statement earlier, so the struct borrow is `E0382`.

## Result

Eighteen guards across `edit_diff.rs`, `edit.rs` and `cyrup-tui`, each proven by reverting the
behaviour, watching the named test fail, and restoring the file byte-identically. Every reachable
branch the feature introduced is executed by at least one of them; the single unreachable arm
(`partial_batch_message`'s `None`) is documented as defensive.

`cargo test -p cyrup-tools` 368 green, `-p cyrup-tui` 1342 green, `cargo clippy --workspace
--all-targets` exits 0, changed files rustfmt-clean.
