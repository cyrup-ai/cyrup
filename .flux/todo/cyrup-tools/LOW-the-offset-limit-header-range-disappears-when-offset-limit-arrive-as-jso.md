---
title: The :offset-limit header range disappears when offset/limit arrive as JSON floats
priority: LOW
tool: read
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: exec
status: in-progress
updated: 2026-08-27
---

# The :offset-limit header range disappears when offset/limit arrive as JSON floats

## Core objective

A model that emits `{"path":"f.txt","offset":2.0,"limit":3.0}` gets the right *bytes* back — the
execute path already accepts floats — but the transcript header silently drops the range and prints
`read f.txt` where pi prints `read f.txt:2-4`. The window that was actually read becomes invisible.

The cause is a type distinction that does not exist upstream. `read_line_range`
([tool_args.rs:58-69](../../../crates/cyrup-tui/src/transcript/tool_args.rs)) extracts both numbers
with `Value::as_i64`, which is `None` for every `serde_json` float; pi never has an integer to lose,
because `JSON.parse` hands it an IEEE-754 double for `2` and for `2.0` alike.

**The fix is one function**: read the numbers as the doubles they always were, do the arithmetic in
`f64`, and render each with the `String(n)` fold JS applies when a number lands in a template
literal. This is a renderer change in `cyrup-tui`, not a `cyrup-tools` change.

## What pi does

[pi read.ts](../../../tmp/pi/packages/coding-agent/src/core/tools/read.ts):

* **`:22-23`** the schema — `Type.Number`, never `Type.Integer`, no `minimum`:

  ```ts
  offset: Type.Optional(Type.Number({ description: "Line number to start reading from (1-indexed)" })),
  limit: Type.Optional(Type.Number({ description: "Maximum number of lines to read" })),
  ```

* **`:73-78`** `formatReadLineRange` — the whole feature, and it performs **no coercion at all**:

  ```ts
  function formatReadLineRange(args: ReadRenderArgs | undefined, theme: Theme): string {
      if (args?.offset === undefined && args?.limit === undefined) return "";
      const startLine = args.offset ?? 1;
      const endLine = args.limit !== undefined ? startLine + args.limit - 1 : "";
      return theme.fg("warning", `:${startLine}${endLine ? `-${endLine}` : ""}`);
  }
  ```

  Three things to carry across verbatim:

  1. **There is no integer/float split to port.** `JSON.parse("2")` and `JSON.parse("2.0")` produce
     the identical double `2`, and `` `${2}` `` is `"2"`. `{"offset":2.0}` is not a special case
     upstream — it is *the same input* as `{"offset":2}`, which is exactly why pi has no code for it.
  2. `startLine + args.limit - 1` is **double arithmetic**, not integer arithmetic. A non-integral
     `offset` reaches the header unrounded: `{"offset":2.5,"limit":3}` renders `:2.5-4.5`. Pi does
     **not** truncate here — `Math.trunc`/`Math.max` appear only in the execute path (`:278-288`),
     never in the header — so the header may legitimately disagree with the window that was read.
     Reproducing that disagreement is parity; "fixing" it is a redesign.
  3. `endLine ? …` is a **truthiness** test on a Number, not a presence test. An end line that
     computes to `0` is falsy and the `-<end>` half is dropped: `{"offset":1,"limit":0}` renders
     `:1`, not `:1-0`.

* **`:80-83`** `formatReadCall` — the plain header appends the range after the path:

  ```ts
  function formatReadCall(args: ReadRenderArgs | undefined, theme: Theme, cwd: string): string {
      const pathDisplay = renderToolPath(str(args?.file_path ?? args?.path), theme, cwd);
      return `${theme.fg("toolTitle", theme.bold("read"))} ${pathDisplay}${formatReadLineRange(args, theme)}`;
  }
  ```

* **`:146-168`** `formatCompactReadCall` — both compact branches call the same
  `formatReadLineRange(args, theme)` (`:156`, `:165`), between the label and the expand hint. One fix
  therefore lands on every read header pi has.

## What cyrup does today

[tool_args.rs:57-69](../../../crates/cyrup-tui/src/transcript/tool_args.rs) — the only implementation
of the range suffix in the workspace:

```rust
/// `formatReadLineRange` (read.ts:67-72): `:<start>` or `:<start>-<end>` from `offset`/`limit`.
pub(super) fn read_line_range(args: &Value) -> Option<String> {
    let offset = args.get("offset").and_then(Value::as_i64);
    let limit = args.get("limit").and_then(Value::as_i64);
    if offset.is_none() && limit.is_none() {
        return None;
    }
    let start = offset.unwrap_or(1);
    Some(match limit {
        Some(l) => format!(":{start}-{}", start + l - 1),
        None => format!(":{start}"),
    })
}
```

`serde_json` 1.0.150 (`Cargo.lock`) implements `Number::as_i64` as `N::Float(_) => None`, so
`json!(2.0)` yields `None` for both fields, the `offset.is_none() && limit.is_none()` guard fires,
and the function returns `None`. Both call sites then append nothing:

* [tool_builtin.rs:22-24](../../../crates/cyrup-tui/src/transcript/tool_builtin.rs) — the plain
  `read <path>` header;
* [tool_args.rs:224-226](../../../crates/cyrup-tui/src/transcript/tool_args.rs) — the collapsed
  `[skill] …` / `read resource …` header, inside `compact_read_call`.

Two secondary divergences fall out of the same expression and are corrected by the same replacement:
`start + l - 1` is integer arithmetic (pi's is not), and `:1-0` is emitted where pi's falsy `endLine`
prints `:1`.

The doc comment's citation is stale: `formatReadLineRange` sits at **`read.ts:73-78`**, not
`:67-72`. Correcting it is this task's, per the sibling brief.

### Why the renderer has to parse the raw arguments

There is no already-coerced value to borrow instead:

* `ToolRun::args` ([entry.rs:174-176](../../../crates/cyrup-tui/src/transcript/entry.rs)) is
  documented and used as *"the raw tool-call arguments … the path/command/pattern/offset/limit/… each
  tool's header is built from"* — the unnormalized model JSON, mirroring pi's `renderCall(args)`;
* `ReadDetails` ([details.rs](../../../crates/cyrup-tools/src/details.rs)) carries only `truncation`,
  never the resolved offset/limit, so the result payload cannot supply them either.

### Why `jsnum` is not the answer

The execute path deliberately accepts floats — `ReadInput` declares
`offset: Option<f64>` / `limit: Option<f64>`
([read.rs:21-22](../../../crates/cyrup-tools/src/tools/read.rs)) and folds them through
`crate::jsnum::to_count` at [read.rs:170](../../../crates/cyrup-tools/src/tools/read.rs) and
[read.rs:191](../../../crates/cyrup-tools/src/tools/read.rs) — and an earlier reading of this gap
proposed reusing that fold in the renderer. **Do not.** Two independent reasons:

1. `jsnum` is `pub(crate) mod jsnum;`
   ([lib.rs:22](../../../crates/cyrup-tools/src/lib.rs)) and does not cross the crate boundary.
   Making it public to serve a header would export an internal coercion primitive for no gain.
2. More importantly it would be **wrong**. `jsnum::to_integer` is ECMA-262 `ToIntegerOrInfinity`, the
   coercion index-taking builtins apply — which is what the *execute* path performs and what the
   *header* deliberately does not. Truncating in the header would render `:2` where pi renders
   `:2.5`, introducing a divergence while closing one.

The correct primitive is JS `String(n)` on a double, and this crate already spells it — inline, once —
at the bash `timeout` header ([tool_builtin.rs:235-238](../../../crates/cyrup-tui/src/transcript/tool_builtin.rs)):

```rust
// `${timeout}s` (bash.ts:204): JS renders an integer number without a trailing `.0`.
let disp = if t.fract() == 0.0 { format!("{}", t as i64) } else { format!("{t}") };
```

Extract that, use it in both places, and the two headers cannot drift apart.

## Required implementation

### 1. `tool_args.rs` — the `String(n)` fold

Add immediately above `read_line_range`
([tool_args.rs:57](../../../crates/cyrup-tui/src/transcript/tool_args.rs)):

```rust
/// JS `String(n)` for a `Number` that came out of `JSON.parse` — the fold a template literal
/// applies when a double is interpolated (`` `:${startLine}` ``, read.ts:77).
///
/// Rust's `Display` for `f64` is already the shortest round-tripping form, so `2.0` prints `2` and
/// `2.5` prints `2.5`, exactly as JS does. `Debug` is NOT — it would print `2.0` — so the `{}`
/// spelling here is load-bearing. The single value the two disagree on is negative zero, which JS
/// prints as `0`; JSON can carry `-0.0`, so it is handled.
///
/// This is deliberately NOT `cyrup_tools::jsnum::to_integer`: that is `ToIntegerOrInfinity`, the
/// coercion the READ path applies to pick a line window (read.ts:278-288). The HEADER interpolates
/// the number as given, and a fractional `offset` reaches the screen unrounded upstream.
pub(super) fn js_number(n: f64) -> String {
    // `String(-0) === "0"`; Rust's `Display` would print `-0`. Covers `+0.0` too.
    if n == 0.0 {
        return "0".to_string();
    }
    format!("{n}")
}
```

### 2. `tool_args.rs` — `read_line_range`

**Current** ([tool_args.rs:57-69](../../../crates/cyrup-tui/src/transcript/tool_args.rs)):

```rust
/// `formatReadLineRange` (read.ts:67-72): `:<start>` or `:<start>-<end>` from `offset`/`limit`.
pub(super) fn read_line_range(args: &Value) -> Option<String> {
    let offset = args.get("offset").and_then(Value::as_i64);
    let limit = args.get("limit").and_then(Value::as_i64);
    if offset.is_none() && limit.is_none() {
        return None;
    }
    let start = offset.unwrap_or(1);
    Some(match limit {
        Some(l) => format!(":{start}-{}", start + l - 1),
        None => format!(":{start}"),
    })
}
```

**Replacement** (the citation is corrected to `read.ts:73-78`, where the function actually sits):

```rust
/// `formatReadLineRange` (read.ts:73-78): `:<start>` or `:<start>-<end>` from `offset`/`limit`.
///
/// ```ts
/// if (args?.offset === undefined && args?.limit === undefined) return "";
/// const startLine = args.offset ?? 1;
/// const endLine = args.limit !== undefined ? startLine + args.limit - 1 : "";
/// return theme.fg("warning", `:${startLine}${endLine ? `-${endLine}` : ""}`);
/// ```
///
/// Upstream has no integer type to lose: `JSON.parse` yields an IEEE-754 double for `2` and for
/// `2.0` alike, so both spellings are literally the same value by the time this runs and both
/// render `:2`. [`Value::as_f64`] is that same "is this a JSON number" test — it answers `Some` for
/// `Number::PosInt`, `NegInt` and `Float` alike, where `as_i64` answers `None` for every float — so
/// it, and not `as_i64`, is the extractor. It is also the more faithful one at the top of the range:
/// `as_f64` narrows `9007199254740993` to `9007199254740992`, which is precisely what `JSON.parse`
/// does with the same literal.
///
/// The arithmetic stays in `f64` because `startLine + args.limit - 1` is double arithmetic
/// upstream; a fractional `offset` reaches the header unrounded there and must here.
pub(super) fn read_line_range(args: &Value) -> Option<String> {
    let offset = args.get("offset").and_then(Value::as_f64);
    let limit = args.get("limit").and_then(Value::as_f64);
    if offset.is_none() && limit.is_none() {
        return None;
    }
    let start = offset.unwrap_or(1.0);
    // `endLine ? …` is a JS TRUTHINESS test on a Number, not a presence test: an end line that
    // computes to zero (`{"offset":1,"limit":0}`) is falsy upstream and the `-<end>` half is
    // dropped. `NaN` is falsy for the same reason and is excluded here for the same reason.
    let end = limit.map(|l| start + l - 1.0).filter(|e| *e != 0.0 && !e.is_nan());
    Some(match end {
        Some(e) => format!(":{}-{}", js_number(start), js_number(e)),
        None => format!(":{}", js_number(start)),
    })
}
```

Nothing about the presence guard changes: a key that is absent, JSON `null`, or a non-number still
yields `None` from the extractor and so still counts as absent. That is a *separate* divergence from
pi's `=== undefined` check (upstream, `{"offset":null}` renders `:1`), it predates this gap, it is
shared with every other header in the file, and it is not this task's — do not fold it in.

### 3. `mod.rs` — re-bind the new helper

`tool_builtin.rs` reaches transcript-internal helpers through its own `use super::*;`, so `js_number`
must join the re-export list at
[mod.rs:84-87](../../../crates/cyrup-tui/src/transcript/mod.rs):

```rust
use tool_args::{
    compact_read_call, compact_read_classification, js_number, key_hint_spans, more_lines_hint,
    push_search_path, read_line_range, str_arg, tool_path_span, StrArg,
};
```

### 4. `tool_builtin.rs` — retire the duplicate fold

**Current** ([tool_builtin.rs:235-238](../../../crates/cyrup-tui/src/transcript/tool_builtin.rs)):

```rust
    if let Some(t) = run.args.get("timeout").and_then(Value::as_f64).filter(|t| *t != 0.0) {
        // `${timeout}s` (bash.ts:204): JS renders an integer number without a trailing `.0`.
        let disp = if t.fract() == 0.0 { format!("{}", t as i64) } else { format!("{t}") };
        spans.push(Span::styled(format!(" (timeout {disp}s)"), theme.muted_style()));
    }
```

**Replacement**:

```rust
    if let Some(t) = run.args.get("timeout").and_then(Value::as_f64).filter(|t| *t != 0.0) {
        // `${timeout}s` (bash.ts:204) — the same `String(n)` fold the read range uses; the `±0`
        // case `js_number` handles is already excluded by the filter above.
        spans.push(Span::styled(format!(" (timeout {}s)", js_number(t)), theme.muted_style()));
    }
```

This is a de-duplication, not a behaviour change: `f64`'s `Display` already prints `120` for `120.0`,
so every timeout the header can show renders byte-identically.

## Exact rendered output

The suffix is one `Span` in `warning_style()`, appended after the path (plain header) or after the
label (compact header). For a read of `f.txt`:

| `args` | header, after the fix | header, today |
| --- | --- | --- |
| `{"path":"f.txt"}` | `read f.txt` | `read f.txt` |
| `{…,"offset":2}` | `read f.txt:2` | `read f.txt:2` |
| `{…,"offset":2.0}` | `read f.txt:2` | `read f.txt` |
| `{…,"limit":3}` | `read f.txt:1-3` | `read f.txt:1-3` |
| `{…,"limit":3.0}` | `read f.txt:1-3` | `read f.txt` |
| `{…,"offset":2,"limit":3}` | `read f.txt:2-4` | `read f.txt:2-4` |
| `{…,"offset":2.0,"limit":3.0}` | `read f.txt:2-4` | `read f.txt` |
| `{…,"offset":2,"limit":3.0}` | `read f.txt:2-4` | `read f.txt:2` |
| `{…,"offset":2.5,"limit":3}` | `read f.txt:2.5-4.5` | `read f.txt` |
| `{…,"offset":1,"limit":0}` | `read f.txt:1` | `read f.txt:1-0` |
| `{…,"offset":0}` | `read f.txt:0` | `read f.txt:0` |
| `{…,"offset":-0.0}` | `read f.txt:0` | `read f.txt` |
| `{…,"offset":-1}` | `read f.txt:-1` | `read f.txt:-1` |
| `{…,"offset":null}` | `read f.txt` | `read f.txt` |

Span by span, for `{"path":"f.txt","offset":2.0,"limit":3.0}` collapsed:

| text | style |
| --- | --- |
| `read ` | `tool_title_style()` |
| `f.txt` | `accent_style()` |
| `:2-4` | `warning_style()` |

The compact headers inherit it unchanged, because `compact_read_call` calls the same function
([tool_args.rs:224-226](../../../crates/cyrup-tui/src/transcript/tool_args.rs)): the same arguments
against `x/SKILL.md` render `[skill] x:2-4 (ctrl+o to expand)`, and against `AGENTS.md` render
`read resource AGENTS.md:2-4 (ctrl+o to expand)`.

## Files that change

| file | change |
| --- | --- |
| [crates/cyrup-tui/src/transcript/tool_args.rs](../../../crates/cyrup-tui/src/transcript/tool_args.rs) | new `js_number` helper above `read_line_range`; `read_line_range` extracts with `Value::as_f64`, computes in `f64`, applies the JS truthiness test to `endLine`, and renders both numbers through `js_number`; the doc citation `read.ts:67-72` is corrected to `read.ts:73-78` |
| [crates/cyrup-tui/src/transcript/mod.rs](../../../crates/cyrup-tui/src/transcript/mod.rs) | add `js_number` to the `use tool_args::{…}` list at `:84-87` |
| [crates/cyrup-tui/src/transcript/tool_builtin.rs](../../../crates/cyrup-tui/src/transcript/tool_builtin.rs) | the bash `timeout` header at `:235-238` uses `js_number` instead of its own inline fold; no behaviour change |

Nothing else moves. In particular:

* **`crates/cyrup-tools/` is untouched.** The execute path is already correct — `ReadInput` takes
  `Option<f64>` ([read.rs:21-22](../../../crates/cyrup-tools/src/tools/read.rs)) and folds through
  `jsnum` — and [jsnum.rs](../../../crates/cyrup-tools/src/jsnum.rs) stays `pub(crate)`. This gap is
  entirely a display-layer gap despite the task's `cyrup-tools` filing.
* The `limit` suffixes on the **grep / ls / find** headers
  ([tool_builtin.rs](../../../crates/cyrup-tui/src/transcript/tool_builtin.rs) `:329`, `:356`,
  `:375`) share the `as_i64` pattern and the same float blindness. They are three other tools with
  three other upstream sources and are **out of scope here** — leave all three exactly as they are.
* [entry.rs](../../../crates/cyrup-tui/src/transcript/entry.rs) and
  [details.rs](../../../crates/cyrup-tools/src/details.rs) are untouched: the renderer keeps reading
  the raw model arguments, as pi's `renderCall(args)` does.

## Coordination with the sibling docs task

[LOW-pi-s-third-compact-read-header-kind-docs-is-not-implemented.md](./LOW-pi-s-third-compact-read-header-kind-docs-is-not-implemented.md)
adds the `docs` arm to `compact_read_classification` and turns `CompactRead::kind` into a
`CompactReadKind` enum, in the same file. The two tasks touch **disjoint functions** and compose with
no further work:

* that task owns `CompactRead`, `compact_read_classification`, `compact_read_call` and
  `docs_classification`; this task owns `read_line_range` and `js_number`. Neither edits the other's;
* `compact_read_call` already calls `read_line_range` unconditionally, so whichever lands second
  needs nothing: a `docs` header with `{"path":"docs/x.md","offset":2.0,"limit":3.0}` renders
  `read docs docs/x.md:2-4`;
* that brief explicitly assigns the `read.ts:67-72` → `read.ts:73-78` citation correction here. It is
  §2 above. Do not also apply it from there.
* neither task adds the `CompactReadKind` enum twice — it belongs to the sibling.

## Definition of done

Observable behaviour of the transcript header, for a `read` tool call whose arguments are as given:

1. `{"path":"f.txt","offset":2.0,"limit":3.0}` renders `read f.txt:2-4` — identical, character for
   character, to what `{"path":"f.txt","offset":2,"limit":3}` renders.
2. `{"path":"f.txt","offset":2.0}` renders `read f.txt:2`, and `{"path":"f.txt","limit":3.0}` renders
   `read f.txt:1-3`.
3. Every integer-spelled case renders exactly as it does today: `:2`, `:1-3`, `:2-4`, `:0`, `:-1`,
   and no suffix at all when neither key is present.
4. `{"path":"f.txt","offset":2.5,"limit":3}` renders `read f.txt:2.5-4.5` — the fractional value
   reaches the screen unrounded, matching upstream, even though the window actually read starts at
   line 2.
5. `{"path":"f.txt","offset":1,"limit":0}` renders `read f.txt:1`, no longer `read f.txt:1-0`.
6. `{"path":"f.txt","offset":-0.0}` renders `read f.txt:0`, never `read f.txt:-0`.
7. A key that is absent, JSON `null`, a string, or any other non-number still produces no suffix —
   unchanged from today.
8. The compact headers carry the same suffix from the same arguments: `x/SKILL.md` renders
   `[skill] x:2-4 (ctrl+o to expand)` and `AGENTS.md` renders
   `read resource AGENTS.md:2-4 (ctrl+o to expand)`.
9. Expanding a read (`Ctrl+O`) shows the plain `read <path>:<range>` header with the same suffix, and
   the file body below it, exactly as it does for integer arguments today.
10. A bash call with `{"timeout":120}` still renders ` (timeout 120s)` and one with `{"timeout":1.5}`
    still renders ` (timeout 1.5s)`.
11. The grep, ls and find headers render byte-identically to before this change for every input.
12. Behaviour that pi does NOT have is not introduced — this is a parity task, not a redesign.
