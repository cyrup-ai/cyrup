---
stage: new
status: pending
type: research
updated: 2026-08-29 02:44
---

# `edit` tool — brittle whitespace/newline matching makes multi-edit batches fail repeatedly

> **This is a research write-up, not an implementation task.** The ask is to document the
> symptoms precisely and collect how *stronger* edit/patch libraries and agent harnesses
> solve the same class of problem — so a human can decide what (if anything) cyrup should
> adopt. **Do not "fix" the matcher as part of this task.** See §5 for the parity
> constraint that governs any eventual change.

---

## 0. One-paragraph statement

cyrup's `edit` tool replaces text by **exact string match** (with a narrow fuzzy fallback).
Every `edits[].oldText` must reproduce the target region byte-for-byte, *including newline
placement and leading indentation*. In practice an agent editing a lot of code reconstructs
`oldText` from memory or from a slightly-reflowed prior read, gets a newline or an indent
subtly wrong, and the edit fails. Worse, a multi-edit call is **all-or-nothing**: one
mismatched edit throws away every other (correct) edit in the same call. This happened
**multiple times in a single session** while editing a Markdown file. pi has the same
behaviour — cyrup's port is faithful — so this is a design-level ergonomics problem shared
with the upstream, not a cyrup regression.

---

## 1. Observed symptoms (from a real session)

All of these are reproducible; the first two are transcribed from an actual failed call.

### (a) A single wrong edit rejects the whole batch, and only the first failure is named

A `subagent`-style `edit` call with **five** edits was submitted against a Markdown file.
Four of the five `oldText` blocks matched exactly. The fifth (`edits[4]`) had been
hand-reconstructed with **different line-wrap boundaries** than the file on disk — the same
words, but newlines fell in different places. Result:

```
Could not find edits[4] in <file>. The oldText must match exactly including all
whitespace and newlines.
```

The call is **atomic**: the four correct edits were *not* applied. The error names only
`edits[4]`; if two edits had been wrong it would still only report the first. The recovery
was to re-`read` the exact bytes, rebuild the failing `oldText` character-for-character, and
resubmit the entire batch. This is the dominant failure mode when editing prose/Markdown
where soft-wrap is cosmetic and an agent naturally normalizes it.

### (b) Reflowed / re-wrapped prose never matches, even though it is "the same text"

Moving a newline (re-wrapping a paragraph to a different column, joining two short lines,
splitting a long one) changes which characters sit on which line. The fuzzy fallback
(§2) trims **trailing** whitespace per line and folds Unicode punctuation, but it does
**not** move or normalize newlines — so a reflowed block is simply *not found*. Markdown and
comments are where this bites hardest because their line breaks carry no semantics.

### (c) Leading-indentation mismatches fail

The fuzzy pass does `str::trim_end` (trailing only). Leading indentation is significant to
the match. Copying a snippet at the wrong indent depth, or tabs-vs-spaces drift, produces
*not found* even when the visible code is identical.

### (d) The uniqueness requirement and the exactness requirement pull against each other

`oldText` must match **exactly one** occurrence (`occurrences > 1` → a duplicate-region
error). To make a short snippet unique, the agent pads `oldText` with surrounding context —
which enlarges the exact-match surface and therefore the chance of a whitespace/newline
mismatch. So the two constraints actively fight: *be unique* pushes toward more text, *match
exactly* punishes more text.

### (e) The failure is silent about *where* the mismatch is

The error says "must match exactly including all whitespace and newlines" but does not show
the nearest near-miss, the diff between the supplied `oldText` and the closest region, or
which character/line first diverged. The agent must re-read and eyeball. A near-miss diff
would turn a multi-round retry into a one-round fix.

---

## 2. What the matcher does today (the baseline to improve on)

Faithful port of pi's `edit-diff.ts`. Core in
[`cyrup-tools/src/tools/edit_diff.rs`](../../crates/cyrup-tools/src/tools/edit_diff.rs),
driver in [`edit.rs`](../../crates/cyrup-tools/src/tools/edit.rs).

1. **Exact substring first** — `content.find(old_text)`
   ([`edit_diff.rs:291-299`](../../crates/cyrup-tools/src/tools/edit_diff.rs),
   `fuzzy_find_text`; pi `fuzzyFindText`, `edit-diff.ts:206-244`).
2. **Fuzzy fallback** — `normalize_for_fuzzy` on **both** buffers, then `find`
   ([`edit_diff.rs:263-281`](../../crates/cyrup-tools/src/tools/edit_diff.rs); pi
   `edit-diff.ts:36`). The normalization is exactly:
   - `NFKC` Unicode normalization;
   - **per-line `trim_end`** (trailing whitespace only — *not* leading);
   - curly quotes → straight, en/em/figure dashes + minus → `-`, assorted Unicode spaces
     (`\u00A0`, `\u2002-\u200A`, `\u202F`, `\u205F`, `\u3000`) → ASCII space.
   - It does **not** touch newlines, indentation, or interior run-length of spaces.
3. **Uniqueness** — `count_occurrences` must be exactly 1, else a duplicate error
   ([`edit_diff.rs:330-346`](../../crates/cyrup-tools/src/tools/edit_diff.rs); pi
   `countOccurrences`, `edit-diff.ts:252-256`).
4. **Overlap check** across the sorted matches
   ([`edit_diff.rs:582-593`](../../crates/cyrup-tools/src/tools/edit_diff.rs)).
5. **All-or-nothing apply** — the batch loop returns `Err` on the first not-found/duplicate
   edit ([`edit_diff.rs:561-577`](../../crates/cyrup-tools/src/tools/edit_diff.rs),
   `apply_edits_to_normalized_content`). No partial application, no per-edit result vector.
6. Line endings (CRLF/LF) and BOM are normalized for matching and restored on write
   ([`edit_diff.rs:27-63`](../../crates/cyrup-tools/src/tools/edit_diff.rs)), so those are
   *not* part of this problem — the gap is interior newlines and indentation.

The exact error strings live at
[`edit_diff.rs:497-513`](../../crates/cyrup-tools/src/tools/edit_diff.rs)
(`err_not_found` / `err_duplicate` / `err_empty`).

**So the fuzzy layer already handles the "smart quotes / Unicode space / trailing
whitespace" class — and nothing else.** The unmet classes are: newline/re-wrap
differences, leading-indentation differences, interior whitespace-run differences, and the
batch atomicity ergonomics.

---

## 3. Upstream provenance (why this is not a cyrup bug)

pi's `edit` behaves identically — cyrup's doc comments cite `edit-diff.ts` line-for-line at
every step above, and the `[CYRUP-DELTA]` notes in `fuzzy_find_text`/`count_occurrences`
are about empty-needle edge cases, **not** about the whitespace matching strategy. Both pi
`edit.ts` copies exist:
[`pi/packages/coding-agent/src/core/tools/edit.ts`](../../../pi/packages/coding-agent/src/core/tools/edit.ts)
(the live one) and
[`pi/packages/agent/src/harness/tools/edit.ts`](../../../pi/packages/agent/src/harness/tools/edit.ts).
Any research conclusion should note whether the improvement is something pi would also want
(candidate for an upstream-shaped change) or a cyrup-only ergonomic delta.

---

## 4. The research question

**How do stronger edit/patch systems make "apply this change" robust to whitespace,
newline placement, and indentation drift — without silently applying the wrong edit?**
Collect concrete mechanisms, their failure modes, and their false-positive risk. Suggested
lines of inquiry (leads, not a prescription — evaluate, compare, and cite sources):

- **Fuzzy patch application with a bounded fuzz factor.** GNU `patch --fuzz`, `git apply
  --recount` / `--whitespace=fix` / `--ignore-whitespace`, and how they bound the risk of a
  wrong hunk (context lines, offsets, hunk headers). What "fuzz" costs in false applies.
- **`diff-match-patch` `patch_apply`** (Myers diff + `Match_Threshold` / `Match_Distance`):
  a tunable similarity threshold rather than exact match. How it decides "close enough".
- **Aider's edit formats** — the `search/replace` block format and its *flexible* matching
  (leading-whitespace-insensitive, "…" elision, whole-function replacement), plus its
  `udiff` format. Aider has written extensively on why exact match fails for LLM edits and
  what they do instead; find and summarize their approach and measured results.
- **OpenAI `apply_patch` / the `*** Begin Patch` V4A format** and Anthropic's
  `str_replace_editor` / text-editor tool — what matching latitude they document, and how
  they report a miss.
- **Anchor-based / line-range editing** — match on stable anchors (unique start/end lines,
  or explicit line numbers) and replace the span between, decoupling the *locator* from the
  *full body* so interior reflow does not break the match.
- **Indentation-normalized matching** — dedent both needle and haystack to a common margin,
  match, then re-indent the replacement (how editors' "smart paste"/reindent do it).
- **Structural / AST-aware editing** — tree-sitter, `ast-grep`, `comby`: match on syntax
  nodes so whitespace is irrelevant by construction. What it costs (per-language grammars)
  and where it does/doesn't apply (prose/Markdown vs code).
- **Fast-apply models** (Morph, Relace, Cursor's apply) — delegating "merge this edit into
  this file" to a model instead of a string matcher. Latency/cost/accuracy trade-offs.

For each mechanism, capture: (1) what drift it tolerates, (2) how it *bounds* false applies
(the real danger — applying an edit to the wrong place is worse than failing), (3) whether it
degrades gracefully to a helpful near-miss diagnostic, and (4) whether it composes with a
*multi-edit batch* (partial success + per-edit results, vs all-or-nothing).

Also evaluate the two ergonomics fixes that are independent of the matcher and cheap:
- **Per-edit results instead of atomic-batch** — apply the edits that match, report the ones
  that don't, so one bad edit does not discard four good ones. (Check whether this is safe
  given the current offset/overlap model, and whether pi would accept the same.)
- **Near-miss diagnostics** — on not-found, show the closest region and a small diff so the
  agent fixes it in one round instead of re-reading and guessing.

---

## 5. Constraints on any eventual change (for whoever acts on the research)

- **Parity first.** cyrup's stated goal is behavioural equivalence with pi
  (`CLAUDE.md`). The matcher is a faithful port; a unilateral divergence must be a
  documented `[CYRUP-DELTA]` with rationale, or better, an upstream-shaped change. Prefer
  options that pi could also adopt.
- **False applies are the cardinal sin.** Any added latitude must not silently replace the
  wrong region. A fuzzy match that lands elsewhere is far worse than an honest "not found".
  Threshold-based schemes must state their false-positive bound.
- **No-panic / clippy policy** applies to any Rust that lands
  (`unwrap_used`/`expect_used`/`panic`/`indexing_slicing` are denied; `cargo clippy
  --workspace --all-targets` must exit 0).
- This file is **research only** — the deliverable is a comparison write-up and a
  recommendation, not a patch to `edit_diff.rs`.

---

## 6. Definition of done (for the research task)

A document that: (1) catalogues the mechanisms in §4 with sources, (2) for each, states the
drift tolerated / false-apply bound / diagnostic quality / batch behaviour, (3) recommends a
concrete direction for cyrup+pi ranked by value-vs-risk, and (4) explicitly calls out which
options are pi-compatible vs cyrup-only. No source files changed.
