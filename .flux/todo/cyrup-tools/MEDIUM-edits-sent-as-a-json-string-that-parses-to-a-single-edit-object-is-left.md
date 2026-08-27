---
title: Edits sent as a JSON string that parses to a single edit object is left unwrapped
priority: MEDIUM
tool: edit
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: new
status: done
updated: 2026-08-27
---

# Edits sent as a JSON string that parses to a single edit object is left unwrapped

## What pi does

In `prepareEditArguments` the string branch accepts BOTH shapes: `if (Array.isArray(parsed)) { args.edits = parsed; } else if (isSingleEditInput(parsed)) { args.edits = [parsed]; }` (/home/user/cyrup/tmp/pi/packages/coding-agent/src/core/tools/edit.ts:127-132). So `edits: "{\"oldText\":\"a\",\"newText\":\"b\"}"` becomes one edit.

## What cyrup-tools does

/home/user/cyrup/crates/cyrup-tools/src/tools/edit.rs:103-108 adopts the parsed value only when `parsed.is_array()`; a parsed object is discarded and `edits` stays a JSON string. The doc comment at edit.rs:88-93 claims a 1:1 port of edit.ts:94-118 but only mirrors the array half.

## User-visible impact

Stringified single-edit payloads fail with `Edit tool input is invalid. edits must contain at least one replacement.` in cyrup while pi applies the edit.

## Parity action

Extend the string branch in `normalize_args`: when the parsed value is not an array but is an object with string `oldText`/`newText`, store `[parsed]` as `edits`.

## Why this gap is real

An adversary agent was tasked with **refuting** this finding by locating the capability in the Rust under another name. It could not:

> Genuine gap, confirmed after searching all of cyrup-tools, cyrup-core and the agent preflight. normalize_args (edit.rs:99-125) is the ONLY normalizer for the edit tool (wired via prepare_arguments at edit.rs:166 and re-applied in execute at edit.rs:205); no other module in cyrup-tools/cyrup-core/cyrup-agent touches `edits`. Its string branch (edit.rs:103-108) is gated on `parsed.is_array()`, so a stringified single edit object is parsed and then discarded, leaving `edits` a JSON String. Pi's equivalent (edit.ts:125-136) accepts both shapes: `Array.isArray(parsed) -> args.edits = parsed` else `isSingleEditInput(parsed) -> args.edits = [parsed]`. The adjacent pi branch for a RAW (non-stringified) single edit object, `else if (isSingleEditInput(args.edits)) args.edits = [args.edits]` (edit.ts:134-136), is likewise absent — ripgrep for is_single_edit/SingleEdit/single_edit across the Rust returns nothing. Nothing upstream compensates: cyrup-provider/src/validate.rs:423-437 `coerce_array` rejects any non-array with `expected array, got ...` and never JSON-parses a string, so the agent preflight fails the call before execute; a caller bypassing preflight hits execute's edits_ok check (edit.rs:210-218) and gets the exact literal "Edit tool input is invalid. edits must contain at least one replacement." Existing tests only cover the array half (tests/tools.rs:403-406 and 465-477 use a stringified ARRAY); there is no stringified-object case. The doc comment at edit.rs:88-93 cites edit.ts:94-118/102-107, stale line numbers versus the current pi prepareEditArguments at edit.ts:116-147, i.e. the port predates pi adding the single-object handling.

## Definition of done

1. The capability described under *Parity action* is implemented in `crates/cyrup-tools`.
2. A test pins the new behaviour against the pi semantics quoted above.
3. `cargo check --workspace --all-targets` and `cargo clippy` stay clean.
4. Behaviour that pi does NOT have is not introduced — this is a parity task, not a redesign.
