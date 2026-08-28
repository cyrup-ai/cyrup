---
title: "CYRUP-DELTA capability gap at crates/cyrup-tools/src/tools/read.rs:221"
priority: MEDIUM
crate: cyrup-tools
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: aug
status: done
updated: 2026-08-28 02:08
---

# Capability gap: `read`'s negative-`limit` window

Classified a **capability gap** — a caller can observe a difference — by the audit
that reviewed all 87 `CYRUP-DELTA` markers against pi at `e8682309`.

**This was never authorized as an accepted divergence.** The marker was written by an
agent. Nobody decided it was acceptable. It is filed here so it is a decision rather
than an artifact.

Marker location: `crates/cyrup-tools/src/tools/read.rs`, inside `ReadTool::execute`, the
comment block immediately above `let end = match input.limit { … }` in the text branch.
(Anchor by that symbol — line 221 has drifted.)

---

## Ground truth: what pi does at `e8682309`

Reference: `tmp/pi/packages/coding-agent/src/core/tools/read.ts` @ `e8682309`.

```ts
// read.ts:288-290
const endLine = Math.min(startLine + limit, allLines.length);
selectedContent = allLines.slice(startLine, endLine).join("\n");
userLimitedLines = endLine - startLine;
```

```ts
// read.ts:313-317 — the "user limit stopped early" branch
} else if (userLimitedLines !== undefined && startLine + userLimitedLines < allLines.length) {
    const remaining = allLines.length - (startLine + userLimitedLines);
    const nextOffset = startLine + userLimitedLines + 1;
    outputText = `${truncation.content}\n\n[${remaining} more lines in file. Use offset=${nextOffset} to continue.]`;
}
```

> **Citation correction.** The audit brief and the in-tree comment both cite
> `read.ts:282` for the `endLine` expression. At `e8682309` that expression is at
> **read.ts:288**, the `slice` at **289**, `userLimitedLines` at **290**, `startLine` at
> **278**, and the out-of-bounds `throw` at **282**. See open question **OQ-4**.

`endLine` is an **unclamped signed value**. Two separate consumers key off it, and they
normalise it differently — that is the whole of the gap:

1. **The slice.** `Array.prototype.slice` resolves its `end` argument as
   `relativeEnd < 0 ? max(len + relativeEnd, 0) : min(relativeEnd, len)`, then takes
   `count = max(final − k, 0)`. So a negative `endLine` **counts from the end of the
   file** and returns a real, usually large window. A negative `limit` that leaves
   `endLine` in `[0, startLine)` instead returns nothing.
2. **The continuation notice.** `userLimitedLines = endLine − startLine` keeps the
   **raw, unnormalised** `endLine`; `startLine + userLimitedLines` telescopes straight
   back to `endLine`. So the notice can quote a `remaining` **larger than the file** and
   an `offset=` that is **zero or negative**.

### Node-verified oracle (10-line file `line1…line10`, no trailing newline, `total = 10`)

Replayed against pi's exact expressions:

| args | pi output |
|---|---|
| `{limit: -1}` | `line1…line9` + `\n\n[11 more lines in file. Use offset=0 to continue.]` |
| `{limit: -5}` | `line1…line5` + `\n\n[15 more lines in file. Use offset=-4 to continue.]` |
| `{offset: 3, limit: -5}` | `line3…line7` + `\n\n[13 more lines in file. Use offset=-2 to continue.]` |
| `{offset: 4, limit: -2}` | `""` + `\n\n[9 more lines in file. Use offset=2 to continue.]` |
| `{offset: 8, limit: -5}` | `""` + `\n\n[8 more lines in file. Use offset=3 to continue.]` |

Note the last two rows: when `|limit| ≤ startLine` pi's window is **also empty** — but
the *notice* still differs from cyrup's, because pi's `offset=` is `endLine + 1`
(`start + limit + 1`) where cyrup's is `start + 1`.

### Negative `offset` is NOT part of this gap

`startLine = offset ? Math.max(0, offset − 1) : 0` (read.ts:278) clamps at zero in pi
itself, and `Math.max(0, −0.5)` is `0`, not `−0.5`. cyrup's
`to_count(offset.map_or(0.0, |o| (o − 1.0).max(0.0)))` reproduces that exactly for every
integral input. **Negative `offset` is already at parity — do not touch it.** The
existing `read_accepts_float_and_negative_numeric_params` assertion for `offset: -5`
stays as-is.

---

## What cyrup does today

```rust
let end = match input.limit {
    Some(l) => crate::jsnum::to_count(start as f64 + l).clamp(start, total),
    None => total,
};
```

`jsnum::to_count` (`crates/cyrup-tools/src/jsnum.rs`) is `to_integer` (the ECMA-262
`ToIntegerOrInfinity` truncate-toward-zero fold) followed by *floor at 0*. That floor is
correct at its other three call sites — each has a pi-side `Math.max(0, …)` or an
equivalent behind it — but `read`'s `endLine` has **no** such guard, so `to_count` +
`.clamp(start, total)` collapses every negative `limit` to `end == start`: an empty
window, and a notice pointing back at `start + 1`.

**What a caller sees.** For any negative `limit`, pi returns a real (possibly large)
slice of the file and cyrup returns **no content at all**. The model can reach this:
`limit` is a bare `Type.Number` with no `minimum` (read.ts:23) and pi never validates
tool arguments (`tool-definition-wrapper.ts:16-18`), so `limit: -5` is an input both
implementations accept and answer differently.

---

## Decision required

One of:

1. **Close it** — bring cyrup to pi's behaviour. *(Prescribed below.)*
2. **Accept it** — David explicitly accepts the divergence; the marker stays and is
   annotated as authorized, with the reason.
3. **Reshape it** — the divergence is right but the current form is wrong.

Do not silently keep option 2 by leaving the marker as-is; that is how this became a
backlog in the first place.

### The argument FOR David, if he wants to consider accepting

Stated once, in full, so it is a decision and not an omission — the close plan below is
the prescription either way:

- pi's negative-`limit` output is arguably a **latent JS bug**: `offset=0` and
  `offset=-4` are not values pi's own `read` will accept on a follow-up call (`offset: 0`
  is falsy → start of file; `offset: -4` → also start of file), so the continuation
  notice is self-inconsistent, and `remaining` exceeds the file's line count.
- Cyrup's current behaviour is the "safe" reading of a nonsense argument.

**Against accepting** (why the close path is prescribed): the *content* difference is
what matters, not the notice. pi hands the model most of the file; cyrup hands it an
empty string. A model that emits `limit: -1` gets a working read from pi and a dead end
from cyrup, and the difference is not observable to the model as an error — it just sees
an empty file. Reproducing a quirk is cheap; the divergence is not. Cyrup's stated
posture elsewhere in this file (see the `offset`, `NaN`, macOS-variant and errno
comments) is byte-for-byte reproduction of pi including its infelicities.

---

## Prescribed change (the CLOSE path)

### 1. `crates/cyrup-tools/src/tools/read.rs` — `ReadTool::execute`, text branch

Replace the `let end = match input.limit { … };` block (and its `[CYRUP-DELTA]` comment)
with a computation that keeps the raw `endLine` alongside the slice bound. Anchor: the
statement immediately preceding `let window: Vec<&str> = lines.get(start..end)…`.

```rust
// Pi: `const endLine = Math.min(startLine + limit, allLines.length)` (read.ts:288), then
// `allLines.slice(startLine, endLine)` (read.ts:289) and
// `userLimitedLines = endLine - startLine` (read.ts:290).
//
// `endLine` is UNCLAMPED and signed, and its two consumers normalise it differently, which
// is why it is kept here instead of being folded straight into one `usize`:
//
//  * the SLICE applies `Array.prototype.slice`'s `relativeEnd` rule — a negative end is
//    resolved as `max(len + end, 0)`, counting from the END of the array, then
//    `count = max(final - k, 0)`. So `limit: -1` on a 10-line file returns lines 1..9, while a
//    negative `limit` that leaves `endLine` in `[0, start)` selects nothing.
//  * the CONTINUATION NOTICE keeps `endLine` RAW (`startLine + userLimitedLines`
//    telescopes back to it), so it quotes a `remaining` that can exceed the file's line
//    count and an `offset=` that can be zero or negative.
let total_i = i64::try_from(total).unwrap_or(i64::MAX);
let start_i = i64::try_from(start).unwrap_or(i64::MAX);
// `Math.min(startLine + limit, allLines.length)`, with `limit` folded through
// `ToIntegerOrInfinity` here rather than inside `slice` — `start` is already integral, so
// the fold is exact for every integral `limit` and the two orders agree.
let end_raw: Option<i64> = input
    .limit
    .map(|l| start_i.saturating_add(crate::jsnum::to_integer(l)).min(total_i));
// `slice`'s `relativeEnd` resolution, then `count = max(final - k, 0)` as a clamp to `start`.
let end: usize = match end_raw {
    Some(e) => {
        let resolved = if e < 0 { (total_i + e).max(0) } else { e };
        usize::try_from(resolved).unwrap_or(0).clamp(start, total)
    }
    None => total,
};
```

`crate::jsnum::to_integer` is already `pub(crate)` and already documented as
`ToIntegerOrInfinity` — **use it, do not add a new helper**, and **do not change
`to_count`**: its three other callers (`read`'s `offset`, `ls`, `find`) each have a
pi-side floor-at-zero behind them and must keep it.

### 2. Same file — the "more lines in file" branch

Replace:

```rust
} else if end < total {
    let remaining = total - end;
    out.push_str(&format!(
        "\n\n[{remaining} more lines in file. Use offset={} to continue.]",
        end + 1
    ));
}
```

with:

```rust
} else if let Some(e) = end_raw
    && e < total_i
{
    // Pi: `remaining = allLines.length - (startLine + userLimitedLines)` and
    // `nextOffset = startLine + userLimitedLines + 1` (read.ts:315-316), where
    // `startLine + userLimitedLines` telescopes back to the RAW `endLine`. Both go straight
    // into the template literal upstream, so a negative `endLine` yields a `remaining` above
    // the file's line count and a zero-or-negative `offset=`. The `end_raw.is_some()` test is
    // Pi's `userLimitedLines !== undefined`, i.e. "the caller passed a limit".
    let remaining = total_i - e;
    out.push_str(&format!(
        "\n\n[{remaining} more lines in file. Use offset={} to continue.]",
        e + 1
    ));
}
```

For every non-negative `limit` this is identical to the code it replaces (`end_raw == end`
and `end_raw.is_some()` ⟺ `end < total` is unreachable when `limit` is `None`, because
`end == total` there). Let-chains are already used in this workspace
(`cyrup-ext-subagents/src/tui/fleet_transcript.rs`), so the `if let … && …` form is fine.

### 3. Delete the `[CYRUP-DELTA]` marker

It is the only justification for the divergence and it is now false. Remove the
`[CYRUP-DELTA]:` sentence; keep the surrounding pi citation, corrected to `read.ts:288`.

### Ordering is unchanged

Pi's `truncateHead` branch runs **before** the user-limit branch (read.ts:296-313), so a
negative-`limit` window large enough to trip the 2000-line / 50KB limits still produces
the `[Showing lines …]` notice, not the `[N more lines …]` one. cyrup already has that
ordering — do not reorder. `truncateHead("")` reports `truncated: false,
firstLineExceedsLimit: false` (truncate.ts:87-101), so the empty-window sub-case falls
through to the notice branch in both implementations.

---

## `cyrup-tui` interaction — checked, no change needed

`crates/cyrup-tui/src/transcript/tool_args.rs::read_line_range` is the port of
`formatReadLineRange` (read.ts:73-78). It was verified against pi for negative `limit`:

- pi: `startLine = args.offset ?? 1`, `endLine = startLine + limit - 1`, rendered as
  `:${startLine}${endLine ? "-" + endLine : ""}` — the `endLine ?` is a JS **truthiness**
  test, so `endLine === 0` drops the range half.
- cyrup: identical, including the `!= 0.0 && !is_nan()` filter and `f64` arithmetic.

`{"limit": -5}` renders `:1--5` in both. **The header is already pi-faithful and this fix
does not touch it.** The header is computed from the *arguments*, not from the tool's
output, so the two are independent.

The only other transcript sites that mention read's notices are
`cyrup-tui/src/transcript/tool_result.rs:157` (a `bash` footer stripper, unrelated) and
`cyrup-ext-subagents/src/tui/fleet_transcript.rs:831` (a `contains("[Showing lines")`
truncation flag, unaffected). Neither parses `offset=` or `remaining`.

Optional, cheap: add `{"path":"x","limit":-5}` → `:1--5` to the `read_line_range` cases in
`cyrup-tui` so the header's negative branch is pinned rather than merely correct.

---

## Test that fails without the change

Amend the existing test — **do not add a new file**. It currently pins the divergent
behaviour *and* misstates pi in its comment ("Pi's unclamped `startLine + limit` selects
nothing" — it does not).

**File:** `crates/cyrup-tools/src/tests/tools.rs`
**Test:** `read_accepts_float_and_negative_numeric_params`
**Fixture already present:** `f.txt` = `line1…line10`, no trailing newline ⇒ `total = 10`.

Replace the `neg_limit` block, whose current assertion is

```rust
assert_eq!(first_text(&neg_limit), "\n\n[10 more lines in file. Use offset=1 to continue.]");
```

with the five Node-verified cases from the oracle table above, each asserted with
`assert_eq!(first_text(&r), …)` on the full string:

| args | expected `first_text` |
|---|---|
| `{"path":"f.txt","limit":-1}` | `"line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\n\n[11 more lines in file. Use offset=0 to continue.]"` |
| `{"path":"f.txt","limit":-5}` | `"line1\nline2\nline3\nline4\nline5\n\n[15 more lines in file. Use offset=-4 to continue.]"` |
| `{"path":"f.txt","offset":3,"limit":-5}` | `"line3\nline4\nline5\nline6\nline7\n\n[13 more lines in file. Use offset=-2 to continue.]"` |
| `{"path":"f.txt","offset":4,"limit":-2}` | `"\n\n[9 more lines in file. Use offset=2 to continue.]"` |
| `{"path":"f.txt","offset":8,"limit":-5}` | `"\n\n[8 more lines in file. Use offset=3 to continue.]"` |

Every one of the five fails against today's code (rows 1-3 return `""` + a wrong notice;
rows 4-5 return the right emptiness with the wrong `offset=`/`remaining`). Rows 4 and 5
are the ones that prove the notice is driven by the **raw** `endLine` and not by the
clamped slice bound — keep both.

Keep the test's existing non-negative assertions unchanged; they are the no-regression
half (`offset: 2.0, limit: 3.0` ≡ `offset: 2, limit: 3`; `offset: -5` ⇒ whole file;
`offset: 99.0` ⇒ the out-of-bounds error text). Also update the block's comment to state
pi's actual rule.

---

## Additional divergences found while researching

Recorded here per the no-descoping rule. **None of these is dismissed**; each is an open
question below.

**AD-1 — fractional `limit` reaches pi's notice unrounded.**
`userLimitedLines` is a JS double, so `{"limit": 2.5}` on the 10-line file makes pi emit
`[7.5 more lines in file. Use offset=3.5 to continue.]` (Node-verified). Cyrup folds
through `to_integer` before formatting and emits `[8 more lines … offset=3]`. The
*content* is identical (`slice` folds `2.5 → 2` in both). The prescribed fix above does
**not** close this: reproducing it needs JS `Number::toString` formatting, and the only
such helper in the tree is `js_number` in
`crates/cyrup-tui/src/transcript/tool_args.rs`, which is `pub(super)` inside a crate that
`cyrup-tools` does not (and must not) depend on.

**AD-2 — fractional `offset` reaches pi's notices unrounded, and can make pi throw.**
`startLine = Math.max(0, offset - 1)` keeps the fraction: `{"offset": 2.5}` gives
`startLine = 1.5`, so `startLineDisplay = 2.5` and pi's `[Showing lines 2.5-…]` /
`Offset 2.5 is beyond…` differ from cyrup's integral rendering. Worse, the
`firstLineExceedsLimit` branch indexes `allLines[1.5]` → `undefined` →
`Buffer.byteLength(undefined, "utf-8")` **throws a TypeError**, which pi's catch
re-`reject`s as a tool error. cyrup uses `window.first()` and returns the note instead.
Same family as AD-1; same missing helper; plus a "do we reproduce a pi crash?" question.

**AD-3 — the `read.ts:NNN` citations in `read.rs` are stale.**
Against `e8682309` they are mixed: `read.ts:271 → 278`, `:275 → 282`, `:282 → 288`,
`:238 → 245`, `:241 → 248`, `:321-324 → 329-330`, while `:246`, `:249` and `:325` are
exact. The file therefore carries citations from at least two pi revisions. Every comment
this task touches should be corrected; the rest of the file is a separate sweep.

---

## Open questions for David

- **OQ-1 (AD-1/AD-2).** Does cyrup reproduce pi's *fractional* renderings in read's
  notices? That needs a shared JS `Number::toString`. Options: (a) lift `js_number` from
  `cyrup-tui/src/transcript/tool_args.rs` into `cyrup-core` and have both crates use it;
  (b) duplicate it in `cyrup-tools`; (c) file AD-1/AD-2 as their own parity task. **Not
  decided here.** The prescribed fix is integral-exact and independent of the answer.
- **OQ-2 (AD-2).** If yes to OQ-1: does cyrup also reproduce pi's `TypeError` on a
  fractional `offset` that reaches the `firstLineExceedsLimit` branch, or is that the one
  place cyrup stays safe?
- **OQ-3.** Should read's continuation `offset=` ever be sanitised? pi can emit
  `offset=0` and `offset=-4`, neither of which pi's own `read` will honour as written.
  Closing this gap makes cyrup emit them too. Deliberate, and stated so David can say
  otherwise.
- **OQ-4 (AD-3).** Authorise a citation-refresh sweep over `read.rs` (and, if the drift
  is workspace-wide, the other tool ports) against `e8682309`?

Log these to `.flux/todo/parity-gaps/MEDIUM-open-questions-from-gap-closure.md` as well.

---

## Definition of done

1. `read`'s window and continuation notice match pi at `e8682309` for every **integral**
   `limit`, negative included, per the five-row oracle table.
2. `read_accepts_float_and_negative_numeric_params` carries the five new assertions and
   fails on the pre-change code.
3. The `[CYRUP-DELTA]` marker at the `endLine` computation is **deleted**, and the pi
   citations in the comments it touches are corrected to `read.ts:288-290` / `:313-317`.
4. `jsnum::to_count` and its other three call sites are untouched; `offset` handling is
   untouched.
5. `cyrup-tui`'s `read_line_range` is untouched (verified already at parity).
6. No behaviour regression in `cyrup-tools`: the non-negative-`limit`, `offset`, image,
   truncation and errno tests in `crates/cyrup-tools/src/tests/tools.rs` still pass.
7. AD-1, AD-2 and AD-3 are carried forward as open questions, not closed silently.
