---
title: Edits sent as a bare single edit object is rejected instead of wrapped into a one-element array
priority: MEDIUM
tool: edit
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: new
status: done
updated: 2026-08-27
---

# Edits sent as a bare single edit object is rejected instead of wrapped into a one-element array

## What pi does

`prepareEditArguments` wraps a single edit OBJECT into a one-element array: `} else if (isSingleEditInput(args.edits)) { args.edits = [args.edits]; }` (/home/user/cyrup/tmp/pi/packages/coding-agent/src/core/tools/edit.ts:134-136, predicate at :74-81). The comment at :123-124 states this exists because real models ("Opus 4.6, GLM-5.1") emit `edits: {oldText, newText}` instead of an array. So `{path, edits: {oldText:"a", newText:"b"}}` executes as one replacement.

## What cyrup-tools does

`normalize_args` in /home/user/cyrup/crates/cyrup-tools/src/tools/edit.rs:99-125 handles only two shapes: `edits` as a JSON *string* that parses to an array (:103-108) and top-level `oldText`/`newText` (:111-122). There is no branch that tests whether `edits` is itself an object with string `oldText`/`newText`. `execute` then hits the guard at edit.rs:210-218 (`edits` must be a non-empty array) and errors. Grepped /home/user/cyrup/crates/cyrup-tools/src for any other single-edit unwrapping (`single_edit`, `is_object()`) — none exists; edit.rs is the only normalizer.

## User-visible impact

A model that emits `edits` as a single object gets `Edit tool input is invalid. edits must contain at least one replacement.` from cyrup where pi silently accepts and performs the edit — a hard tool failure and wasted turn for a shape pi was specifically patched to tolerate.

## Parity action

In `normalize_args`, after the edits-as-string branch, add: if `obj["edits"]` is a JSON object whose `oldText` and `newText` are both strings, replace it with a one-element array containing that object.

## Why this gap is real

An adversary agent was tasked with **refuting** this finding by locating the capability in the Rust under another name. It could not:

> Genuinely absent. normalize_args (crates/cyrup-tools/src/tools/edit.rs:99-125) has only two branches: `edits` as a JSON string that parses to an ARRAY (:103-108, gated on parsed.is_array()) and top-level oldText/newText appended to edits (:111-122). Nothing tests whether `edits` is itself an object with string oldText/newText. Searched all of crates/cyrup-tools/src, cyrup-agent/src, cyrup-core/src: is_object/as_object in non-test cyrup-tools source hits only edit.rs:100 and isolation/policy.rs:269; `edits` appears nowhere in cyrup-agent/src or cyrup-core/src; edit.rs is the only prepare_arguments impl for the edit tool. The generic coercion layer does not cover it either: the preflight (crates/cyrup-agent/src/agent/run/tools/preflight.rs:38-46) runs prepare_arguments then cyrup_provider::validate_tool_call, whose coerce_array (crates/cyrup-provider/src/validate.rs:423-437) hard-rejects any non-array ("expected array, got object") with no single-value wrapping anywhere in that file. Root cause: edit.rs was ported against pi @v0.83.0 (its doc cites edit.ts:94-118); the vendored pi is 0.84.3, where prepareEditArguments gained the single-object branch at :123-136. Two corrections to the claim: (1) through the normal agent path the model actually receives "schema validation failed at `$.edits`: expected array, got object" from the preflight, not edit.rs:214's "edits must contain at least one replacement." — that message only fires for direct execute callers bypassing the preflight; (2) the gap is slightly wider: pi also wraps when a STRINGIFIED edits parses to a single object (edit.ts:130-132), which edit.rs:103-108's is_array() gate likewise drops. Severity medium rather than higher: the call fails loudly with a retryable error the model sees next turn, no silent wrong behaviour and no file corruption, but it is a hard tool failure on a shape pi was specifically patched to tolerate from shipping models (Opus 4.6, GLM-5.1).

## Definition of done

1. The capability described under *Parity action* is implemented in `crates/cyrup-tools`.
2. A test pins the new behaviour against the pi semantics quoted above.
3. `cargo check --workspace --all-targets` and `cargo clippy` stay clean.
4. Behaviour that pi does NOT have is not introduced — this is a parity task, not a redesign.
