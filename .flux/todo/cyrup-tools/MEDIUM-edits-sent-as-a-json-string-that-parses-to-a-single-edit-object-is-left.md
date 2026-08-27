---
title: Edits sent as a JSON string that parses to a single edit object is left unwrapped
priority: MEDIUM
tool: edit
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: aug
status: done
updated: 2026-08-27
---

# Edits sent as a JSON string that parses to a single edit object is left unwrapped

## This task is subsumed — read this section before doing anything

This finding is **one arm of a single code change** that is already fully specified in the sibling
task
[edits sent as a bare single edit object is rejected instead of wrapped](./MEDIUM-edits-sent-as-a-bare-single-edit-object-is-rejected-instead-of-wrapped-i.md).

Pi implements both shapes in one nine-line `if/else if` at
[pi edit.ts:125-136](../../../tmp/pi/packages/coding-agent/src/core/tools/edit.ts) that shares one
predicate, `isSingleEditInput`. Cyrup must mirror it in one place, the `edits` block of
[`normalize_args`](../../../crates/cyrup-tools/src/tools/edit.rs) at
[edit.rs:101-108](../../../crates/cyrup-tools/src/tools/edit.rs). There is no way to land this task's
arm (the **string** that parses to a single edit object,
[edit.ts:130-131](../../../tmp/pi/packages/coding-agent/src/core/tools/edit.ts)) without rewriting
that block, and rewriting that block is exactly what closes the sibling's arm (the **bare** object,
[edit.ts:134-135](../../../tmp/pi/packages/coding-agent/src/core/tools/edit.ts)) too.

**Ordering:**

- If the sibling has already been executed, **this task requires no code change at all.** Verify the
  four rows in *Definition of done* below and close it.
- If this task is executed first, implement the sibling's *Required change* **in full and verbatim** —
  the `is_single_edit` predicate and the whole replacement block. Do not write a string-only
  coercion; a second, separate coercion for the bare-object arm would then have to be bolted on and
  the two would fight over the same `obj.get("edits")` match.

The predicate, the borrow-checker shaping of the replacement block, the doc-comment corrections, the
"why not serde" argument, the out-of-scope list and the full acceptance matrix all live in the
sibling and are **not** repeated here. What follows is only what is genuinely residual to this task:
the exact semantics of the **string** arm and its failure modes.

## The string arm — verified against pi 0.84.3

Vendored at [tmp/pi/packages/coding-agent](../../../tmp/pi/packages/coding-agent/package.json)
(`"version": "0.84.3"`).
[pi edit.ts:125-133](../../../tmp/pi/packages/coding-agent/src/core/tools/edit.ts):

```ts
	if (typeof args.edits === "string") {
		try {
			const parsed = JSON.parse(args.edits);
			if (Array.isArray(parsed)) {
				args.edits = parsed;
			} else if (isSingleEditInput(parsed)) {
				args.edits = [parsed];
			}
		} catch {}
	} else if (isSingleEditInput(args.edits)) {
```

Four outcomes, and only two of them assign anything:

1. **The string does not parse.** `JSON.parse` throws, the empty `catch {}` at `:133` swallows it,
   and `args.edits` is left **as the original string**. Nothing is logged, nothing is thrown, no
   partial value is written. The call then dies downstream on the ordinary non-array rejection.
2. **It parses to an array.** `args.edits = parsed` — adopted **verbatim, unvalidated**. Pi does not
   inspect the elements: `"[]"` becomes `[]` and is rejected a few lines later by
   [`validateEditInput` (edit.ts:149-154)](../../../tmp/pi/packages/coding-agent/src/core/tools/edit.ts)
   for being empty; `"[1,2]"` becomes `[1,2]` and dies at the typed decode. This half already works
   in cyrup and must not change.
3. **It parses to an object that `isSingleEditInput` accepts** — a non-null, non-array object whose
   `oldText` and `newText` are both `string`. `args.edits = [parsed]`, the wrap **verbatim**: extra
   keys on the object ride along into `edits[0]`. **This is the missing case.**
4. **It parses to anything else** — a scalar, `null`, a nested JSON string, an object missing either
   property or carrying a non-string one. `isSingleEditInput` returns `false` and **nothing is
   assigned**: `args.edits` stays the **original string**, not the parsed value. This is the subtlety
   the implementation must not get wrong — a failed single-edit check does not downgrade `edits` to
   the parsed object, it leaves the string in place.

Two consequences that constrain the Rust:

- **There is no second parse.** `edits: "\"[{\\\"oldText\\\"…}]\""` (double-encoded) parses to a
  *string*, which is neither an array nor a single edit object, so pi leaves the outer string alone.
  Do not add a recursive re-parse; that is behaviour pi does not have.
- **The arm is total in the assignment sense only.** Every non-assigning path is silent and
  value-preserving. Any Rust that returns an error, logs, or inserts a partial value from this arm is
  a divergence.

## What cyrup does today

[edit.rs:101-108](../../../crates/cyrup-tools/src/tools/edit.rs), the whole of the string handling as
it stands:

```rust
        // edits-as-string -> parse, but only adopt the parsed value when it is an array
        // (Pi `if (Array.isArray(parsed)) args.edits = parsed`, edit.ts:104-106).
        if let Some(serde_json::Value::String(s)) = obj.get("edits")
            && let Ok(parsed) = serde_json::from_str::<serde_json::Value>(s)
            && parsed.is_array()
        {
            obj.insert("edits".to_string(), parsed);
        }
```

The `&& parsed.is_array()` conjunct is the gap. Outcome 3 is parsed successfully and then discarded:
the let-chain fails on the third condition, no insert happens, and `edits` remains a JSON string. The
outcome is byte-identical to outcome 4, which is why the bug is invisible until the call is compared
against pi.

`normalize_args` ([edit.rs:99-125](../../../crates/cyrup-tools/src/tools/edit.rs)) is the only
normalizer for this tool — wired as `Tool::prepare_arguments` at
[edit.rs:166-168](../../../crates/cyrup-tools/src/tools/edit.rs) and re-applied inside `execute` at
[edit.rs:205](../../../crates/cyrup-tools/src/tools/edit.rs). Nothing upstream compensates, and a
fix at the serde layer would be unreachable code; the sibling proves both points and they are not
re-argued here.

Through the agent the failure is a **schema** failure, not the tool's own message:
[preflight.rs:38-47](../../../crates/cyrup-agent/src/agent/run/tools/preflight.rs) runs
`prepare_arguments` then `validate_tool_call`, and `coerce_array`
([validate.rs:424-438](../../../crates/cyrup-provider/src/validate.rs)) rejects the surviving string
with ``schema validation failed at `$.edits`: expected array, got string``. Only a caller that
invokes `execute` directly, bypassing the preflight, sees
`Edit tool input is invalid. edits must contain at least one replacement.` from the `edits_ok` guard
at [edit.rs:210-218](../../../crates/cyrup-tools/src/tools/edit.rs).

## Citation corrections

Every reference inherited by this task was checked against the vendored pi 0.84.3 and the current
Rust. The pi line numbers in the original write-up were off; the Rust ones were right.

| Cited as | Actual |
| --- | --- |
| string branch accepts both shapes at edit.ts:127-132 | edit.ts:125-133 — the array adopt is :128-129, the single-object wrap :130-131, the empty `catch {}` :133 |
| bare-object arm at edit.ts:134-136 | edit.ts:134-135 |
| doc comment claims a 1:1 port of edit.ts:94-118 | the doc comment spans edit.rs:88-98 and cites edit.ts:94-118 and edit.ts:102-107; `prepareEditArguments` is actually edit.ts:116-147 and the string branch edit.ts:125-133 |
| `isSingleEditInput` at edit.ts:74-81 | edit.ts:74-81 — correct |
| `validateEditInput` at edit.ts:120-125 | edit.ts:149-154 |
| `coerce_array` at validate.rs:423-437 | validate.rs:424-438; the non-array reject arm is :430-438 |
| edit.rs:103-108, edit.rs:99-125, edit.rs:166, edit.rs:205, edit.rs:210-218 | all correct |

The claim that the model receives `expected array, got string` from the preflight rather than
edit.rs's own literal is **confirmed**.

## Required change

One file changes: [crates/cyrup-tools/src/tools/edit.rs](../../../crates/cyrup-tools/src/tools/edit.rs).

Land the sibling's *Required change* whole — the `is_single_edit` predicate above `normalize_args`,
the replacement `edits` block, and the two stale doc-comment corrections
([edit.rs:88-98](../../../crates/cyrup-tools/src/tools/edit.rs) and the
``(edit.ts:94-118)`` parenthetical at
[edit.rs:58](../../../crates/cyrup-tools/src/tools/edit.rs)). No other file is touched.

Within that block, the two match arms this task owns are:

CURRENT — the let-chain at
[edit.rs:103-108](../../../crates/cyrup-tools/src/tools/edit.rs) shown above, whose
`&& parsed.is_array()` silently discards a parsed single edit object.

REPLACEMENT — the string arm of the sibling's `coerced_edits` match:

```rust
            Some(serde_json::Value::String(s)) => {
                match serde_json::from_str::<serde_json::Value>(s) {
                    Ok(parsed) if parsed.is_array() => Some(parsed),
                    Ok(parsed) if is_single_edit(&parsed) => {
                        Some(serde_json::Value::Array(vec![parsed]))
                    }
                    _ => None,
                }
            }
```

The `_ => None` catch-all is pi's empty `catch {}` **and** its two silent non-assignments in one arm:
`Err(_)` (outcome 1) and `Ok(_)` that is neither an array nor a single edit object (outcome 4) both
yield `None`, and `None` means the caller performs no insert, so `edits` keeps the **original
string**. Never insert `parsed` on the fall-through path.

Guard order is load-bearing and matches pi's `if` / `else if`: `is_array` is tested first, so a
parsed array is adopted verbatim and never reaches the single-edit check.

`serde_json::from_str` stands in for `JSON.parse`. The two agree on every input that can produce a
usable edit: both are strict RFC 8259 parsers, both take last-wins on duplicate keys, neither accepts
trailing content or trailing commas. The one known divergence is a lone surrogate escape such as
`"\ud800"` inside the encoded string, which `JSON.parse` accepts and `serde_json` rejects; that input
is left as a string in cyrup and wrapped in pi, but it can never match file content on either side.
It is accepted, not worked around — do not hand-roll a parser for it.

## Residual acceptance rows

The full matrix is in the sibling. These four rows are the ones this task exists for; all describe
the value of `edits` after `normalize_args`.

| incoming `edits` | after `normalize_args` |
| --- | --- |
| `"{\"oldText\":\"a\",\"newText\":\"b\"}"` | `[{"oldText":"a","newText":"b"}]` |
| `"{\"oldText\":\"a\",\"newText\":\"b\",\"note\":\"x\"}"` | `[{"oldText":"a","newText":"b","note":"x"}]` — verbatim, extras kept |
| `"{\"oldText\":\"a\"}"`, `"{\"oldText\":1,\"newText\":\"b\"}"`, `"null"`, `"7"` | unchanged — still the original string, **not** the parsed value |
| `"not json"` | unchanged — still the original string |

## Definition of done

Observable behaviour, through the real order `prepare_arguments` → `validate_tool_call` → `execute`,
on a file `a.txt` containing `alpha\n`:

1. `{"path":"a.txt","edits":"{\"oldText\":\"alpha\",\"newText\":\"ALPHA\"}"}` rewrites the file to
   `ALPHA\n` and returns the ordinary success result. It no longer returns
   ``schema validation failed at `$.edits`: expected array, got string``.
2. `{"path":"a.txt","edits":"{\"oldText\":\"alpha\",\"newText\":\"ALPHA\",\"note\":\"x\"}"}` also
   rewrites the file to `ALPHA\n`; the extra key rides along harmlessly.
3. Invoking `execute` directly with either of those, bypassing the preflight, performs the edit and
   no longer returns `Edit tool input is invalid. edits must contain at least one replacement.`
4. `edits` of `"not json"`, `"{\"oldText\":\"alpha\"}"`, `"{\"oldText\":1,\"newText\":\"b\"}"`,
   `"null"` and `"7"` still fail, with the same message and at the same layer as before this change —
   the failure names `got string`, proving the original string was preserved rather than replaced by
   the parsed value.
5. `{"path":"a.txt","edits":"[{\"oldText\":\"alpha\",\"newText\":\"ALPHA\"}]"}` and
   `{"path":"a.txt","edits":"[]"}` behave exactly as they do today.
6. The sibling's own definition of done holds as well, since the same block satisfies both.
