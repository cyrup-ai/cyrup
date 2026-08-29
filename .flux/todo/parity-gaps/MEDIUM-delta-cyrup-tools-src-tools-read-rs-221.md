---
title: "CYRUP-DELTA capability gap at crates/cyrup-tools/src/tools/read.rs:221"
priority: MEDIUM
crate: cyrup-tools
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: aug
status: done
updated: 2026-08-28 21:00
---

# `read`'s negative-`limit` window — EXECUTION path

Scope, stated first because this workstream has already confused the two:

| path | file | what it decides | status |
|---|---|---|---|
| **HEADER render** | [`crates/cyrup-tui/src/transcript/tool_args.rs`](../../../crates/cyrup-tui/src/transcript/tool_args.rs) `read_line_range` (line 298) | the `:1--5` text drawn next to `read foo.txt` in the transcript | **fixed earlier today — out of scope, do not touch** |
| **EXECUTION** | [`crates/cyrup-tools/src/tools/read.rs`](../../../crates/cyrup-tools/src/tools/read.rs) `ReadTool::execute` (lines 218-233, 296-302) | **which lines of the file are actually read**, and the continuation notice appended to them | **this task** |

The two are independent: the header is computed from the *arguments*, the execution path from the
*file*. Fixing one does not move the other.

Classified a **capability gap** — a caller can observe a difference — by the audit that reviewed all
87 `CYRUP-DELTA` markers against pi at `e8682309`. **This was never authorized as an accepted
divergence.** The marker was written by an agent; nobody decided it was acceptable.

Marker location: [`crates/cyrup-tools/src/tools/read.rs:221`](../../../crates/cyrup-tools/src/tools/read.rs)
— the `[CYRUP-DELTA]:` sentence inside the comment block at lines 218-224, immediately above
`let end = match input.limit { … }` (line 225). **Verified 2026-08-28: line 221 is still exact, it
has NOT drifted.** (An earlier revision of this task claimed it had; that claim was wrong.)

---

## Ground truth: pi at `e8682309`

Reference: [`tmp/pi/packages/coding-agent/src/core/tools/read.ts`](../../../tmp/pi/packages/coding-agent/src/core/tools/read.ts)
(`git -C tmp/pi rev-parse HEAD` = `e86823096c5bad39e1ca282ec24bc5eb9bec745b`). **Every line number
below was re-read from that file today.**

```ts
// read.ts:284-293
let selectedContent: string;
let userLimitedLines: number | undefined;
// If limit is specified by the user, honor it first. Otherwise truncateHead decides.
if (limit !== undefined) {
    const endLine = Math.min(startLine + limit, allLines.length);    // :288
    selectedContent = allLines.slice(startLine, endLine).join("\n"); // :289
    userLimitedLines = endLine - startLine;                          // :290
} else {
    selectedContent = allLines.slice(startLine).join("\n");
}
```

```ts
// read.ts:313-317 — the "user limit stopped early" branch
} else if (userLimitedLines !== undefined && startLine + userLimitedLines < allLines.length) {
    const remaining = allLines.length - (startLine + userLimitedLines);
    const nextOffset = startLine + userLimitedLines + 1;
    outputText = `${truncation.content}\n\n[${remaining} more lines in file. Use offset=${nextOffset} to continue.]`;
}
```

Other anchors verified in the same file: `startLine` at **:278**, the out-of-bounds `throw` at
**:282**, the `limit` schema property at **:24** (`offset` at **:23**), `truncateHead(...)` at
**:295**, and the three branches that precede the user-limit branch at **:297** / **:302** / **:313**.

`endLine` is an **unclamped signed** JS number. Two consumers key off it and normalise it
differently — that is the entire gap:

1. **The slice** (`:289`). `Array.prototype.slice` resolves `end` as
   `relativeEnd < 0 ? max(len + relativeEnd, 0) : min(relativeEnd, len)`, then takes
   `count = max(final − k, 0)`. So a negative `endLine` **counts from the end of the file** and
   returns a real, usually large window. A negative `limit` that leaves `endLine` in `[0, startLine)`
   instead returns nothing.
2. **The continuation notice** (`:290` → `:315-316`). `userLimitedLines = endLine − startLine` keeps
   the **raw, unnormalised** `endLine`; `startLine + userLimitedLines` telescopes straight back to
   it. So the notice can quote a `remaining` **larger than the file** and an `offset=` that is
   **zero or negative**.

### Node-verified oracle

Replayed today against pi's exact expressions. Fixture: 10-line file `line1…line10`, **no trailing
newline** ⇒ `allLines.length = 10`.

| args | pi output (exact `first_text`) |
|---|---|
| `{limit: -1}` | `line1…line9` + `\n\n[11 more lines in file. Use offset=0 to continue.]` |
| `{limit: -5}` | `line1…line5` + `\n\n[15 more lines in file. Use offset=-4 to continue.]` |
| `{offset: 3, limit: -5}` | `line3…line7` + `\n\n[13 more lines in file. Use offset=-2 to continue.]` |
| `{offset: 4, limit: -2}` | `""` + `\n\n[9 more lines in file. Use offset=2 to continue.]` |
| `{offset: 8, limit: -5}` | `""` + `\n\n[8 more lines in file. Use offset=3 to continue.]` |

Rows 4-5 are load-bearing: when `|limit| ≤ startLine` pi's *window* is also empty, but the *notice*
still differs from cyrup's, because pi's `offset=` is `endLine + 1` (`start + limit + 1`) where
cyrup's is `start + 1`. They are what prove the notice is driven by the **raw** `endLine` and not by
the clamped slice bound.

`truncateHead("")` returns `{ content: "", truncated: false, firstLineExceedsLimit: false }`
([`truncate.ts:87-101`](../../../tmp/pi/packages/coding-agent/src/core/tools/truncate.ts)), so the
empty-window sub-case falls through to the notice branch in both implementations.

### Negative `offset` is NOT part of this gap

`const startLine = offset ? Math.max(0, offset - 1) : 0` (read.ts:**278**) clamps at zero in pi
itself, and `Math.max(0, −0.5)` is `0`. cyrup's `to_count(offset.map_or(0.0, |o| (o − 1.0).max(0.0)))`
([read.rs:206](../../../crates/cyrup-tools/src/tools/read.rs)) reproduces that exactly for every
integral input. **Negative `offset` is already at parity — do not touch it.**

---

## What cyrup does today

[`read.rs:225-229`](../../../crates/cyrup-tools/src/tools/read.rs):

```rust
let end = match input.limit {
    #[allow(clippy::cast_precision_loss)]
    Some(l) => crate::jsnum::to_count(start as f64 + l).clamp(start, total),
    None => total,
};
```

[`crate::jsnum::to_count`](../../../crates/cyrup-tools/src/jsnum.rs) (jsnum.rs:37) is `to_integer`
(the ECMA-262 §7.1.5 `ToIntegerOrInfinity` truncate-toward-zero fold, jsnum.rs:25) followed by a
**floor at 0**. That floor is correct at its other five call sites — each has a pi-side clamp behind
it — but `read`'s `endLine` has **no** such guard, so `to_count` + `.clamp(start, total)` collapses
every negative `limit` to `end == start`: an empty window, and a notice pointing back at `start + 1`.

**What a caller sees.** For any negative `limit`, pi returns a real (possibly large) slice of the
file and cyrup returns **no content at all**. The model can reach this: `limit` is a bare
`Type.Number` with no `minimum` (read.ts:**24**) and pi never validates tool arguments
([`tool-definition-wrapper.ts:16-18`](../../../tmp/pi/packages/coding-agent/src/core/tools/tool-definition-wrapper.ts)),
so `limit: -5` is an input both implementations accept and answer differently. The model cannot even
see it as an error — it just sees an empty file.

---

## Decision: CLOSE. Bring cyrup to pi.

Recorded once, so it is a decision and not an omission. The counter-argument is that pi's
negative-`limit` notice is a latent JS bug — `offset=0` and `offset=-4` are not values pi's own
`read` will honour on a follow-up call (both fall back to the start of the file) — and cyrup's
emptiness is the "safe" reading of a nonsense argument.

It loses to the *content* difference, which is what the model consumes: pi hands it most of the
file, cyrup hands it an empty string, silently. Reproducing a quirk is cheap; the divergence is not.
Cyrup's posture in this very file (the `offset`, `NaN`, macOS-variant and errno comments) is
byte-for-byte reproduction of pi including its infelicities. **Close it.**

---

## Required implementation

One path. Do not substitute an alternative shape.

### 1. [`crates/cyrup-tools/src/tools/read.rs`](../../../crates/cyrup-tools/src/tools/read.rs) — replace lines 218-229

Replace the comment block (218-224) **and** the `let end = match input.limit { … };` statement
(225-229) — the `#[allow(clippy::cast_precision_loss)]` goes with it, because the replacement has no
`start as f64` — with:

```rust
// Pi: `const endLine = Math.min(startLine + limit, allLines.length)` (read.ts:288), then
// `allLines.slice(startLine, endLine)` (read.ts:289) and
// `userLimitedLines = endLine - startLine` (read.ts:290).
//
// `endLine` is UNCLAMPED and signed, and its two consumers normalise it differently, which is
// why the raw value is kept here instead of being folded straight into one `usize`:
//
//  * the SLICE applies `Array.prototype.slice`'s `relativeEnd` rule — a negative end resolves as
//    `max(len + end, 0)`, counting from the END of the array — then `count = max(final - k, 0)`,
//    which is the floor at `start`. So `limit: -1` on a 10-line file returns lines 1..9, while a
//    negative `limit` leaving `endLine` in `[0, start)` selects nothing.
//  * the CONTINUATION NOTICE keeps `endLine` RAW (`startLine + userLimitedLines` telescopes back
//    to it, read.ts:315-316), so it quotes a `remaining` that can exceed the file's line count and
//    an `offset=` that can be zero or negative.
let total_i = i64::try_from(total).unwrap_or(i64::MAX);
let start_i = i64::try_from(start).unwrap_or(i64::MAX);
// `Math.min(startLine + limit, allLines.length)`, with `limit` folded through
// `ToIntegerOrInfinity` here rather than inside `slice`. `start` is integral, so for every input
// that can reach this line the two fold orders agree.
let end_raw: Option<i64> = input
    .limit
    .map(|l| start_i.saturating_add(crate::jsnum::to_integer(l)).min(total_i));
// `slice`'s `relativeEnd` resolution, then `count = max(final - k, 0)` as a floor at `start`.
// Saturating: `to_integer` folds a huge negative `limit` to `i64::MIN`, and `total_i + i64::MIN`
// would otherwise overflow.
let end: usize = match end_raw {
    Some(e) => {
        let resolved = if e < 0 { total_i.saturating_add(e).max(0) } else { e };
        usize::try_from(resolved).unwrap_or(0).clamp(start, total)
    }
    None => total,
};
```

Use [`crate::jsnum::to_integer`](../../../crates/cyrup-tools/src/jsnum.rs) (jsnum.rs:25) — it is
already `pub(crate)` and already documented as `ToIntegerOrInfinity`. **Do not add a new helper, and
do not change `to_count`.** `to_count`'s five other callers each have a pi-side floor-at-zero behind
them and must keep it: [read.rs:206](../../../crates/cyrup-tools/src/tools/read.rs) (`offset`),
[ls.rs:107](../../../crates/cyrup-tools/src/tools/ls.rs),
[find.rs:154](../../../crates/cyrup-tools/src/tools/find.rs), and
[grep.rs:649 and :657](../../../crates/cyrup-tools/src/tools/grep.rs).

### 2. Same file — replace the notice branch, lines 296-302

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
    // `startLine + userLimitedLines` telescopes back to the RAW `endLine`. Both go straight into
    // the template literal, so a negative `endLine` yields a `remaining` above the file's line
    // count and a zero-or-negative `offset=`. `end_raw.is_some()` is Pi's
    // `userLimitedLines !== undefined`, i.e. "the caller passed a limit".
    let remaining = total_i.saturating_sub(e);
    out.push_str(&format!(
        "\n\n[{remaining} more lines in file. Use offset={} to continue.]",
        e + 1
    ));
}
```

Let-chains are already used inside this crate —
[`crates/cyrup-tools/src/isolation/traversal.rs:74-76`](../../../crates/cyrup-tools/src/isolation/traversal.rs) —
and the workspace is edition 2024 / `rust-version = "1.96"` (Cargo.toml:88-89), so `if let … && …`
compiles.

**Equivalence for non-negative `limit`.** `end_raw == Some(min(start + l, total)) == Some(end)`, and
`end_raw.is_some() && e < total_i` ⟺ `end < total` (when `limit` is `None`, `end == total`, so the
old condition was already unreachable). Byte-identical output.

### 3. Delete the `[CYRUP-DELTA]` marker

The sentence at read.rs:221 is the only justification for the divergence and it is now false. It is
deleted as part of replacing lines 218-224; the replacement comment above carries the corrected pi
citations (`read.ts:288-290`, `:315-316`).

### Ordering is unchanged

`truncateHead` (read.ts:295) and its `firstLineExceedsLimit` / `truncated` branches (read.ts:297,
:302) run **before** the user-limit branch (read.ts:313). So a negative-`limit` window large enough
to trip the 2000-line / 50KB limits produces the `[Showing lines …]` notice, not the
`[N more lines …]` one. cyrup already has that ordering (read.rs:240, :266, :296) — **do not
reorder.**

### Note on fold ordering (why the prescription is safe)

pi computes `endLine` in floats and folds via `ToIntegerOrInfinity` inside `slice`; the prescription
folds `limit` first. For integral `start` these disagree only when `m = start + trunc(limit)` is
positive and `limit` has a negative fractional part, in which case pi's slice end is `m − 1` and
cyrup's is `m`. That requires `trunc(limit) ≥ 1` **and** `limit < 0` simultaneously — impossible —
or `m > total`, which needs `start ≥ total`, already rejected by the out-of-bounds error
(read.rs:207-217). **Selected content is therefore identical for every reachable input.** Only the
notice's numerals can differ, and only for a fractional `limit`: see AD-1.

---

## Guard that fails without the change

Amend the existing test — **do not add a new file, do not add a new test fn.**

**File:** [`crates/cyrup-tools/src/tests/tools.rs`](../../../crates/cyrup-tools/src/tests/tools.rs)
**Test:** `read_accepts_float_and_negative_numeric_params` (line 2392)
**Fixture already present:** `f.txt` = `line1…line10`, no trailing newline ⇒ `total = 10`.

Replace the `neg_limit` block (tools.rs:2434-2450), whose current assertion pins the divergence

```rust
assert_eq!(first_text(&neg_limit), "\n\n[10 more lines in file. Use offset=1 to continue.]");
```

and whose comment misstates pi ("Pi's unclamped `startLine + limit` selects nothing" — it does not),
with the five oracle rows, each asserted with `assert_eq!(first_text(&r), …)` on the full string:

| args | expected `first_text` |
|---|---|
| `{"path":"f.txt","limit":-1}` | `"line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\n\n[11 more lines in file. Use offset=0 to continue.]"` |
| `{"path":"f.txt","limit":-5}` | `"line1\nline2\nline3\nline4\nline5\n\n[15 more lines in file. Use offset=-4 to continue.]"` |
| `{"path":"f.txt","offset":3,"limit":-5}` | `"line3\nline4\nline5\nline6\nline7\n\n[13 more lines in file. Use offset=-2 to continue.]"` |
| `{"path":"f.txt","offset":4,"limit":-2}` | `"\n\n[9 more lines in file. Use offset=2 to continue.]"` |
| `{"path":"f.txt","offset":8,"limit":-5}` | `"\n\n[8 more lines in file. Use offset=3 to continue.]"` |

All five fail against today's code: rows 1-3 return `""` plus a wrong notice; rows 4-5 return the
right emptiness with the wrong `offset=` / `remaining`.

Keep the test's existing non-negative assertions unchanged — they are the no-regression half
(`offset: 2.0, limit: 3.0` ≡ `offset: 2, limit: 3`; `offset: -5` ⇒ whole file; `offset: 99.0` ⇒ the
out-of-bounds error text). Update the block's comment to state pi's actual rule.

---

## `cyrup-tui` interaction — checked, no change needed

[`crates/cyrup-tui/src/transcript/tool_args.rs:298`](../../../crates/cyrup-tui/src/transcript/tool_args.rs)
`read_line_range` is the port of `formatReadLineRange` (read.ts:73-78). **It was already reworked
earlier today** and now ports all four of read.ts:74-77's rules as the separate JS operators they
are — the presence gate, `?? 1` nullish, `limit !== undefined` presence, and truthiness on the
computed end — via `js_arg` / `js_truthy` / `js_to_number` / `js_add` in the same file (lines 200,
222, 237, 262). Verified today for the negative case: `{"limit": -5}` renders `:1--5`
(`js_add(1, -5)` = `-4`, `- 1` = `-5`, non-zero ⇒ the range half is kept, `js_number(-5.0)` = `"-5"`),
matching pi.

**The header is already pi-faithful and this fix does not touch it.** Do not edit `tool_args.rs`
under this task. (An earlier revision of this task described the pre-rework `read_line_range` as
"identical, including the `!= 0.0 && !is_nan()` filter and `f64` arithmetic". The truthiness filter
survives; the plain-`f64` arithmetic does not — it is `js_add` now.)

Optional, cheap, and the only permitted tui-side edit: add
`assert_eq!(read_line_range(&json!({"limit": -5})).unwrap(), ":1--5");` to
`read_line_range_ports_all_four_rules` in
[`crates/cyrup-tui/src/transcript/tests/js_arg.rs:64`](../../../crates/cyrup-tui/src/transcript/tests/js_arg.rs)
so the header's negative branch is pinned rather than merely correct.

No other consumer parses read's notices. Verified by grepping the workspace for
`more lines in file` / `Showing lines`: the only other hits are
[`cyrup-tui/src/transcript/tool_result.rs:157`](../../../crates/cyrup-tui/src/transcript/tool_result.rs)
(a **bash** footer stripper — different string, unrelated) and
[`cyrup-ext-subagents/src/tui/fleet_transcript.rs:831`](../../../crates/cyrup-ext-subagents/src/tui/fleet_transcript.rs)
(`text.contains("[Showing lines")`, a boolean truncation flag). Neither parses `offset=` or
`remaining`.

---

## Additional divergences found while researching

Recorded per the no-descoping rule. **None is dismissed**; each is an open question below.

**AD-1 — fractional `limit` reaches pi's notice unrounded.** `userLimitedLines` is a JS double, so
`{"limit": 2.5}` on the 10-line file makes pi emit
`[7.5 more lines in file. Use offset=3.5 to continue.]` (Node-verified today). Cyrup folds through
`to_integer` before formatting and emits `[8 more lines … offset=3]`. The *content* is identical
(`line1\nline2` in both). The prescribed fix does **not** close this and does not make it worse:
reproducing it needs JS `Number::toString`, and the only such helper in the tree is `js_number` in
[`cyrup-tui/src/transcript/tool_args.rs:127`](../../../crates/cyrup-tui/src/transcript/tool_args.rs),
which is `pub(super)` inside a crate `cyrup-tools` does not (and must not) depend on. The same
formatting gap swallows an extreme `limit` such as `-1e30`, where pi prints `1e+30` and the
saturating Rust prints `i64::MAX`; the *content* (an empty window) still matches, and the saturating
arithmetic in §1/§2 is what keeps that case from overflowing in a debug build.

**AD-2 — fractional `offset` reaches pi's notices unrounded, and can make pi throw.**
`startLine = Math.max(0, offset - 1)` (read.ts:278) keeps the fraction: `{"offset": 2.5}` gives
`startLine = 1.5`, so `startLineDisplay = 2.5` and pi's `[Showing lines 2.5-…]` /
`Offset 2.5 is beyond…` differ from cyrup's integral rendering. Worse, the `firstLineExceedsLimit`
branch indexes `allLines[1.5]` → `undefined` → `Buffer.byteLength(undefined, "utf-8")` **throws a
TypeError** (read.ts:299), which pi's catch (read.ts:328-331) re-`reject`s as a tool error. cyrup
uses `window.first()` (read.rs:246) and returns the note instead. Same family as AD-1, same missing
helper, plus a "do we reproduce a pi crash?" question.

**AD-3 — the `read.ts:NNN` citations in `read.rs` are stale, and more broadly than first thought.**
Against `e8682309`, verified line by line today:

| `read.rs` comment | cites | actual at `e8682309` |
|---|---|---|
| :17 | `read.ts:22-23` | `:23-24` |
| :35 | `read.ts:20-24` | `:21-25` |
| :81 | `read.ts:212-214` | `:216-220` |
| :124 | `read.ts:238` | `:245` |
| :126, :143 | `read.ts:241` | `:248` |
| :144 | `read.ts:321-324` | `:328-331` |
| :171 | `read.ts:243` | `:250` |
| :195 | `read.ts:268-269` | `:275-276` |
| :200 | `read.ts:271` | `:278` |
| :204 | `read.ts:283` | `:289` |
| :208 | `read.ts:275` | `:282` |
| :218 | `read.ts:282` | `:288` |
| :242 | `read.ts:290-294,315` | `:297-301, :322` |
| :248 | `read.ts:293` | `:299-300` |
| :276 | `read.ts:300-304` | `:307-311` |
| :286 | `read.ts:303` | `:310` |
| :305 | `read.ts:294-315` | `:301-322` |
| :331 | `read.ts:247-263` | `:254-270` |
| :405 | `read.ts:87-92` | `:93-98` |
| :406 | `read.ts:246` | `:253` |

Exact at `e8682309` and needing no change: `:95` → `read.ts:222`, `:109` → `:232-235`, `:154` →
`:246`, `:165` → `:249`, `:183`/`:256`/`:314` → `:325`, `:335` → `:256` and `:273`, `:419` → `:257`.
(`:64` cites `read.ts:210-211 @v0.83.0` explicitly and is a different revision on purpose.) The file
therefore carries citations from at least two pi revisions. **Only the comments this task rewrites
(lines 218-224 and 296-302) are corrected here**; the rest is a separate sweep — OQ-4. `jsnum.rs:4`
and `tests/tools.rs:2380` carry the same `read.ts:22-23` drift.

---

## Open questions for David

Carried here, in this file. Do not close them silently and do not answer them inside the
implementation.

- **OQ-1 (AD-1/AD-2).** Does cyrup reproduce pi's *fractional* renderings in read's notices? That
  needs a shared JS `Number::toString`. Options: (a) lift `js_number` from
  `cyrup-tui/src/transcript/tool_args.rs` into `cyrup-core` and have both crates use it;
  (b) duplicate it in `cyrup-tools`; (c) file AD-1/AD-2 as their own parity task. **Not decided
  here.** The prescribed fix is integral-exact and independent of the answer.
- **OQ-2 (AD-2).** If yes to OQ-1: does cyrup also reproduce pi's `TypeError` on a fractional
  `offset` that reaches the `firstLineExceedsLimit` branch, or is that the one place cyrup stays
  safe?
- **OQ-3.** Should read's continuation `offset=` ever be sanitised? pi can emit `offset=0` and
  `offset=-4`, neither of which pi's own `read` will honour as written. Closing this gap makes cyrup
  emit them too. Deliberate, and stated so David can say otherwise.
- **OQ-4 (AD-3).** Authorise a citation-refresh sweep over `read.rs` (twenty stale anchors, table
  above) and, if the drift is workspace-wide, the other tool ports, against `e8682309`?

---

## Definition of done

1. `read.rs` lines 218-229 are replaced by the `end_raw` / `end` computation in §1, and lines
   296-302 by the `if let Some(e) = end_raw && e < total_i` branch in §2.
2. The `[CYRUP-DELTA]:` sentence at read.rs:221 no longer exists anywhere in the file, and the two
   comments that replace it cite `read.ts:288-290` and `read.ts:315-316`.
3. `read_accepts_float_and_negative_numeric_params` (tools.rs:2392) asserts all five oracle rows on
   the full `first_text` string, and every one of them fails when run against the pre-change code.
4. `read`'s window and continuation notice match pi at `e8682309` for every **integral** `limit`,
   negative included.
5. `crates/cyrup-tools/src/jsnum.rs` is unmodified; `to_count`'s five other call sites (read.rs:206,
   ls.rs:107, find.rs:154, grep.rs:649, grep.rs:657) are unmodified; `offset` handling at
   read.rs:200-217 is unmodified.
6. `crates/cyrup-tui/src/transcript/tool_args.rs` is unmodified (the header path is already at
   parity). At most, one assertion is added to `read_line_range_ports_all_four_rules` in
   `crates/cyrup-tui/src/transcript/tests/js_arg.rs`.
7. No regression in `cyrup-tools`: the non-negative-`limit`, `offset`, image, truncation and errno
   tests in `crates/cyrup-tools/src/tests/tools.rs` still pass unchanged.
8. AD-1, AD-2 and AD-3 remain open in this file as OQ-1…OQ-4, not closed as part of the fix.
