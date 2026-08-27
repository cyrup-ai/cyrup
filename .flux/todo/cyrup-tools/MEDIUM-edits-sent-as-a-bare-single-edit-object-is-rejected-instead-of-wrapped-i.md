---
title: Edits sent as a bare single edit object is rejected instead of wrapped into a one-element array
priority: MEDIUM
tool: edit
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: exec
status: in-progress
updated: 2026-08-27
---

# Edits sent as a bare single edit object is rejected instead of wrapped into a one-element array

## Core objective

Make the `edit` tool's argument-normalization shim accept the two non-array `edits` shapes that pi
accepts, with pi's exact acceptance rules and pi's exact rejections:

1. `edits` is a **bare JSON object** with string `oldText`/`newText` → wrap into `[obj]`.
2. `edits` is a **JSON string that parses to such an object** → wrap the parsed value into `[parsed]`.

Shape 2 is the subject of the sibling task
[edits sent as a JSON string that parses to a single edit object](./MEDIUM-edits-sent-as-a-json-string-that-parses-to-a-single-edit-object-is-left.md).
Both shapes are fixed by the **one** rewrite of the `edits` block in
[`normalize_args`](../../../crates/cyrup-tools/src/tools/edit.rs) prescribed below, sharing a single
predicate. Whichever task is executed first lands the whole block; the other is then already
satisfied. Do not write two separate coercions.

## Upstream truth — pi 0.84.3

Vendored at [tmp/pi/packages/coding-agent](../../../tmp/pi/packages/coding-agent/package.json)
(`"version": "0.84.3"`).

The predicate — [pi edit.ts:72-81](../../../tmp/pi/packages/coding-agent/src/core/tools/edit.ts):

```ts
type SingleEditInput = { oldText: string; newText: string };

function isSingleEditInput(value: unknown): value is SingleEditInput {
	if (!value || typeof value !== "object" || Array.isArray(value)) {
		return false;
	}

	const edit = value as Record<string, unknown>;
	return typeof edit.oldText === "string" && typeof edit.newText === "string";
}
```

The shim — [pi edit.ts:116-147](../../../tmp/pi/packages/coding-agent/src/core/tools/edit.ts):

```ts
function prepareEditArguments(input: unknown): EditToolInput {
	if (!input || typeof input !== "object") {
		return input as EditToolInput;
	}

	const args = input as Record<string, unknown>;

	// Some models (Opus 4.6, GLM-5.1) send edits as a JSON string instead of an array.
	// Others send a single edit object instead of a one-element edits array.
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
		args.edits = [args.edits];
	}

	const legacy = args as LegacyEditToolInput;
	if (typeof legacy.oldText !== "string" || typeof legacy.newText !== "string") {
		return args as EditToolInput;
	}

	const edits = Array.isArray(legacy.edits) ? [...legacy.edits] : [];
	edits.push({ oldText: legacy.oldText, newText: legacy.newText });
	const { oldText: _oldText, newText: _newText, ...rest } = legacy;
	return { ...rest, edits } as EditToolInput;
}
```

Load-bearing details of these thirty lines:

- The wrap is **verbatim**: `[parsed]` / `[args.edits]`. Extra keys on the object survive; pi checks
  only `oldText` and `newText`.
- `isSingleEditInput` rejects `null`, arrays, and any non-object, and requires **both** properties to
  be `string` — one missing or non-string property is a rejection.
- Every rejection is silent and leaves `args.edits` **exactly as it arrived** (still a string, still
  a malformed object). Nothing throws; the empty `catch {}` at `:133` swallows parse failures.
- The `else if` at `:134` is structural, not semantic: a `string` is never a single-edit object, and
  the string branch always leaves `edits` an array or the original string, so the two arms are
  disjoint under any input.
- The single-edit wrap runs **before** the legacy `oldText`/`newText` append at `:138-146`, so
  `{path, edits:{…}, oldText, newText}` yields **two** edits — the wrapped one first, the legacy pair
  appended.

Downstream, [pi edit.ts:149-154](../../../tmp/pi/packages/coding-agent/src/core/tools/edit.ts)
(`validateEditInput`) still rejects a non-array or empty `edits` with
`Edit tool input is invalid. edits must contain at least one replacement.`

## Current cyrup behaviour

[crates/cyrup-tools/src/tools/edit.rs:99-125](../../../crates/cyrup-tools/src/tools/edit.rs) —
`normalize_args`, the whole of it, as it stands:

```rust
fn normalize_args(mut raw: serde_json::Value) -> serde_json::Value {
    if let Some(obj) = raw.as_object_mut() {
        // edits-as-string -> parse, but only adopt the parsed value when it is an array
        // (Pi `if (Array.isArray(parsed)) args.edits = parsed`, edit.ts:104-106).
        if let Some(serde_json::Value::String(s)) = obj.get("edits")
            && let Ok(parsed) = serde_json::from_str::<serde_json::Value>(s)
            && parsed.is_array()
        {
            obj.insert("edits".to_string(), parsed);
        }
        // legacy single-edit: append the pair whenever BOTH oldText and newText are strings
        // (Pi edit.ts:109-117). A non-string (or absent) oldText/newText leaves the args untouched.
        let both_strings = obj.get("oldText").is_some_and(serde_json::Value::is_string)
            && obj.get("newText").is_some_and(serde_json::Value::is_string);
        if both_strings {
            let old = obj.remove("oldText").unwrap_or(serde_json::Value::Null);
            let new = obj.remove("newText").unwrap_or(serde_json::Value::Null);
            let mut edits = match obj.get("edits") {
                Some(serde_json::Value::Array(a)) => a.clone(),
                _ => Vec::new(),
            };
            edits.push(serde_json::json!({ "oldText": old, "newText": new }));
            obj.insert("edits".to_string(), serde_json::Value::Array(edits));
        }
    }
    raw
}
```

The `parsed.is_array()` conjunct in the let-chain is the entire gap. A parsed **object** is computed
and thrown away; a bare object is never looked at at all.

`normalize_args` is the only normalizer for this tool. It is wired at
[edit.rs:166-168](../../../crates/cyrup-tools/src/tools/edit.rs) as
`Tool::prepare_arguments` and re-applied defensively at
[edit.rs:205](../../../crates/cyrup-tools/src/tools/edit.rs) inside `execute`.

### Where the call actually dies

Through the agent, the failure is a **schema** failure, not the tool's own message.
[preflight.rs:38-47](../../../crates/cyrup-agent/src/agent/run/tools/preflight.rs):

```rust
let prepared = tool.prepare_arguments(Value::Object(call.arguments.clone())).await;
let mut args = match validate_tool_call(tool.parameters(), prepared) {
    Ok(coerced) => coerced,
    Err(e) => {
        return Prep::Immediate(Box::new(self.immediate_error(call, e.to_string(), false)))
    }
};
```

`validate_tool_call` enters at path `"$"`
([validate.rs:56-58](../../../crates/cyrup-provider/src/validate.rs)) and `coerce_array`
([validate.rs:423-437](../../../crates/cyrup-provider/src/validate.rs)) hard-rejects any non-array:

```rust
let arr = match value {
    Value::Array(a) => a,
    other => {
        return Err(ToolValidationError::schema(
            path,
            format!("expected array, got {}", type_name(&other)),
        ));
    }
};
```

with the `Display` at [validate.rs:39](../../../crates/cyrup-provider/src/validate.rs) —
``schema validation failed at `{path}`: {detail}``. So today a model sees:

- bare object → ``schema validation failed at `$.edits`: expected array, got object``
- stringified object → ``schema validation failed at `$.edits`: expected array, got string``

`edit.rs`'s own literal `Edit tool input is invalid. edits must contain at least one replacement.`
([edit.rs:210-218](../../../crates/cyrup-tools/src/tools/edit.rs)) only fires for callers that invoke
`execute` directly, bypassing the preflight seam.

Nothing upstream compensates. `coerce_object`
([validate.rs:362-421](../../../crates/cyrup-provider/src/validate.rs)) never JSON-parses a string
and never wraps a scalar into an array; the `edit` schema at
[edit.rs:59-77](../../../crates/cyrup-tools/src/tools/edit.rs) declares
`edits: { "type": "array" }` with `required: ["path","edits"]`.

## Citation corrections carried out during this augmentation

The line references inherited from the audit and from the source comments were checked against the
vendored pi and corrected:

| Cited as | Actual |
| --- | --- |
| `prepareEditArguments` at edit.ts:94-118 (doc comment, [edit.rs:89](../../../crates/cyrup-tools/src/tools/edit.rs) and [edit.rs:58](../../../crates/cyrup-tools/src/tools/edit.rs)) | edit.ts:116-147 |
| string branch at edit.ts:102-107 / :104-106 | edit.ts:125-133; the array adopt is :128-129 |
| stringified-object wrap at edit.ts:130-132 | edit.ts:130-131 |
| bare-object wrap at edit.ts:134-136 | edit.ts:134-135 |
| legacy append at edit.ts:109-117 | edit.ts:138-146 |
| `isSingleEditInput` at edit.ts:74-81 | correct |
| `validateEditInput` at edit.ts:120-125 | edit.ts:149-154 |
| model comment at edit.ts:100 / :123-124 | edit.ts:123-124 |

The audit's claim that the model receives `expected array, got object` from the preflight rather than
edit.rs's own literal is **confirmed** — see *Where the call actually dies*.

## Why the fix is not at the serde layer

`serde(untagged)`, a `deserialize_with` on `EditInput::edits`, or a hand-written `Deserialize` impl
would be **unreachable code** for both shapes, and must not be written:

- Through the preflight, `validate_tool_call` rejects the call at
  [validate.rs:429-437](../../../crates/cyrup-provider/src/validate.rs) before `execute` is entered,
  so `serde_json::from_value::<EditInput>` at
  [edit.rs:219-220](../../../crates/cyrup-tools/src/tools/edit.rs) never runs.
- For a direct `execute` caller, the `edits_ok` guard at
  [edit.rs:210-218](../../../crates/cyrup-tools/src/tools/edit.rs) — pi's `validateEditInput`,
  which upstream also runs *before* any typed decode — rejects the non-array first, again before
  serde.

Pi performs this coercion in `prepareArguments`, upstream of both gates. Cyrup must perform it in the
same place: `normalize_args`. `EditInput` and `EditOp`
([edit.rs:14-26](../../../crates/cyrup-tools/src/tools/edit.rs)) are **not** modified — they keep
their plain derives, and their default lenience toward unknown fields is what lets a wrapped object
carrying extra keys decode, exactly as pi's verbatim `[parsed]` does.

## Required change

One file changes: [crates/cyrup-tools/src/tools/edit.rs](../../../crates/cyrup-tools/src/tools/edit.rs).

### 1. Add the predicate, immediately above `normalize_args`

```rust
/// Pi `isSingleEditInput` (edit.ts:74-81): a non-null, non-array JSON OBJECT whose `oldText` and
/// `newText` are both strings. Matching `Value::Object` performs pi's whole three-part guard
/// (`!value`, `typeof value !== "object"`, `Array.isArray(value)`) in one pattern. Extra keys are
/// deliberately tolerated — pi checks exactly these two properties and then wraps the value
/// VERBATIM, so anything else the model attached rides along into `edits[0]`.
fn is_single_edit(value: &serde_json::Value) -> bool {
    let serde_json::Value::Object(map) = value else {
        return false;
    };
    map.get("oldText").is_some_and(serde_json::Value::is_string)
        && map.get("newText").is_some_and(serde_json::Value::is_string)
}
```

### 2. Replace the `edits` block inside `normalize_args`

CURRENT ([edit.rs:101-108](../../../crates/cyrup-tools/src/tools/edit.rs)):

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

REPLACEMENT:

```rust
        // Pi edit.ts:125-136, both arms. The replacement value is computed out of line so the
        // shared borrow of `obj` ends before the insert.
        //
        // - `edits` as a JSON string: parse (edit.ts:127), then adopt the parsed value when it is
        //   an array (`args.edits = parsed`, edit.ts:128-129) OR wrap it when it is a single edit
        //   object (`args.edits = [parsed]`, edit.ts:130-131). Everything else — a parse failure
        //   (pi's empty `catch {}`, edit.ts:133), a scalar, an object without both string
        //   properties — leaves `edits` as the ORIGINAL STRING, untouched, exactly as pi does.
        // - otherwise a BARE single edit object is wrapped in place
        //   (`args.edits = [args.edits]`, edit.ts:134-135).
        //
        // Pi's `else if` is structural only: a string is never a single edit object, and the string
        // arm always leaves `edits` an array or a string, so the two arms cannot both apply.
        let coerced_edits: Option<serde_json::Value> = match obj.get("edits") {
            Some(serde_json::Value::String(s)) => {
                match serde_json::from_str::<serde_json::Value>(s) {
                    Ok(parsed) if parsed.is_array() => Some(parsed),
                    Ok(parsed) if is_single_edit(&parsed) => {
                        Some(serde_json::Value::Array(vec![parsed]))
                    }
                    _ => None,
                }
            }
            Some(other) if is_single_edit(other) => {
                Some(serde_json::Value::Array(vec![other.clone()]))
            }
            _ => None,
        };
        if let Some(edits) = coerced_edits {
            obj.insert("edits".to_string(), edits);
        }
```

The legacy `oldText`/`newText` block at
[edit.rs:109-122](../../../crates/cyrup-tools/src/tools/edit.rs) stays **exactly** where it is,
below this block and unmodified — that ordering is pi's (`:125-136` then `:138-146`) and is what
makes `{path, edits:{…}, oldText, newText}` produce two edits in pi's order.

`normalize_args` stays idempotent: re-running it on the output sees `edits` as an array, so both
match arms fall through to `_ => None`. This is required by the double application at
[edit.rs:167](../../../crates/cyrup-tools/src/tools/edit.rs) and
[edit.rs:205](../../../crates/cyrup-tools/src/tools/edit.rs).

### 3. Correct the two stale doc comments in the same file

Replace the `normalize_args` doc comment at
[edit.rs:88-98](../../../crates/cyrup-tools/src/tools/edit.rs) with one that states the shim's real,
now-complete contract and cites pi 0.84.3 correctly:

```rust
/// Normalize legacy shapes into `{ path, edits: [...] }` (R-03-020), a 1:1 port of Pi
/// `prepareEditArguments` (edit.ts:116-147 @0.84.3):
/// - `edits` sent as a JSON string -> parse, adopting an array verbatim (edit.ts:128-129) and
///   wrapping a single edit object into a one-element array (edit.ts:130-131); a parse failure or
///   any other parsed shape leaves the string untouched (pi's empty `catch {}`, edit.ts:133);
/// - `edits` sent as a BARE single edit object -> wrapped into a one-element array
///   (edit.ts:134-135). Pi added both single-object arms after the shapes were observed from
///   shipping models (edit.ts:123-124: "Opus 4.6, GLM-5.1");
/// - whenever BOTH top-level `oldText`/`newText` are strings, APPEND `{oldText,newText}` to the
///   existing `edits` array (or a fresh one), regardless of whether `edits` is already present
///   (edit.ts:143-145: `const edits = Array.isArray(legacy.edits) ? [...legacy.edits] : [];
///   edits.push(...)`). This runs AFTER the wrap, so `{edits:{…}, oldText, newText}` yields two
///   edits, wrapped-first.
///
/// The previous gate (`!obj.contains_key("edits")`) diverged from Pi: input
/// `{path, edits:[], oldText, newText}` made Pi succeed with one edit but made cyrup keep `edits:[]`
/// and fire the empty-array error, and `{edits:[{...}], oldText, newText}` had Pi append an extra
/// edit while cyrup ignored the pair.
```

And in `EditTool::new`, the parenthetical at
[edit.rs:58](../../../crates/cyrup-tools/src/tools/edit.rs) reading
``pi's `prepareEditArguments` (edit.ts:94-118)`` becomes
``pi's `prepareEditArguments` (edit.ts:116-147)``. Nothing else in `new` changes — in particular the
schema at [edit.rs:59-77](../../../crates/cyrup-tools/src/tools/edit.rs) keeps
`edits: { "type": "array" }` and keeps omitting `additionalProperties`, because pi's TypeBox emission
does, and because the shim runs before the schema is applied.

## Resulting acceptance matrix

Every row is pi's behaviour and must become cyrup's. "unchanged" means `normalize_args` leaves the
value byte-identical and the existing rejection path fires with its existing message.

| incoming `edits` | after `normalize_args` |
| --- | --- |
| `[{"oldText":"a","newText":"b"}]` | unchanged |
| `{"oldText":"a","newText":"b"}` | `[{"oldText":"a","newText":"b"}]` |
| `{"oldText":"a","newText":"b","note":"x"}` | `[{"oldText":"a","newText":"b","note":"x"}]` — verbatim, extras kept |
| `"[{\"oldText\":\"a\",\"newText\":\"b\"}]"` | parsed array (already works) |
| `"{\"oldText\":\"a\",\"newText\":\"b\"}"` | `[{"oldText":"a","newText":"b"}]` — the sibling task |
| `"{\"oldText\":\"a\"}"` | unchanged (still the string) |
| `"not json"` | unchanged (still the string) |
| `{"oldText":1,"newText":"b"}` | unchanged (still the object) |
| `{"oldText":"a"}` | unchanged (still the object) |
| `[]` | unchanged |
| `null` | unchanged |
| absent, with top-level string `oldText`/`newText` | `[{oldText,newText}]` (already works) |
| `{"oldText":"a","newText":"b"}` **plus** top-level string `oldText`/`newText` | two edits: the wrapped object, then the appended legacy pair |

## Out of scope — do not touch

- [crates/cyrup-tui/src/app/event_extract.rs:137-170](../../../crates/cyrup-tui/src/app/event_extract.rs)
  (`edit_preview`). It is a port of pi `getRenderablePreviewInput`
  ([pi edit.ts:199-222](../../../tmp/pi/packages/coding-agent/src/core/tools/edit.ts)), which
  handles only an `edits` **array** and the legacy top-level pair and returns `null` for anything
  else. Pi did **not** extend the preview to the single-object shape; neither may cyrup. The preview
  is skipped for one frame and the post-write diff renders as before.
- [crates/cyrup-provider/src/validate.rs](../../../crates/cyrup-provider/src/validate.rs). No
  single-value-to-array wrapping is to be added to the generic coercion layer; pi's coercion is not
  there either, and widening it would change every tool's schema semantics.
- `EditInput`, `EditOp`, the `parameters()` schema, `execute`'s `edits_ok` guard and its literal
  error text — all unchanged.

## Definition of done

Observable behaviour, through the real preflight order (`prepare_arguments` → `validate_tool_call` →
`execute`), on a file `a.txt` containing `alpha\n`:

1. `{"path":"a.txt","edits":{"oldText":"alpha","newText":"ALPHA"}}` rewrites the file to `ALPHA\n`
   and returns the ordinary success result. It no longer returns
   ``schema validation failed at `$.edits`: expected array, got object``.
2. `{"path":"a.txt","edits":"{\"oldText\":\"alpha\",\"newText\":\"ALPHA\"}"}` rewrites the file to
   `ALPHA\n`. It no longer returns
   ``schema validation failed at `$.edits`: expected array, got string``.
3. Invoking `execute` directly with either shape, bypassing the preflight, performs the edit and no
   longer returns `Edit tool input is invalid. edits must contain at least one replacement.`
4. `edits` of `{"oldText":1,"newText":"b"}`, `{"oldText":"a"}`, `"not json"`, `"{\"oldText\":\"a\"}"`,
   `[]` and `null` still fail, with the same messages and at the same layer as before this change —
   no new leniency.
5. `{"path":"a.txt","edits":{"oldText":"alpha","newText":"ALPHA","note":"x"}}` succeeds; the extra
   key rides along harmlessly.
6. `{"path":"a.txt","edits":{"oldText":"al","newText":"AL"},"oldText":"pha","newText":"PHA"}`
   applies two replacements, the wrapped object first and the legacy pair second, and the top-level
   `oldText`/`newText` keys are removed from the normalized arguments.
7. `{"path":"a.txt","edits":[{"oldText":"alpha","newText":"ALPHA"}]}` and
   `{"path":"a.txt","oldText":"alpha","newText":"ALPHA"}` behave exactly as they do today.
8. The TUI's inline edit preview behaves exactly as it does today for every shape above.
